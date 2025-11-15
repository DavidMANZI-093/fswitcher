use chrono::Local;
use futures_util::StreamExt;
use input::event::EventTrait;
use input::event::keyboard::KeyboardEventTrait;
use input::{Event as LibinputEvent, Libinput, LibinputInterface};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use tokio::io::unix::AsyncFd;
use utils::keys::Key;
use zbus::{Message, MessageStream, connection::Builder, proxy};

mod utils;

#[proxy(
    interface = "org.a11y.atspi.Registry",
    default_service = "org.a11y.atspi.Registry",
    default_path = "/org/a11y/atspi/registry"
)]
trait Registry {
    fn register_event(&self, event: &str) -> zbus::Result<()>;
    fn deregister_event(&self, event: &str) -> zbus::Result<()>;
}

// Event polling keys
// const KEY_LIBINPUT: usize = 0;
// const KEY_DBUS: usize = 1;

struct Interface;

impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        OpenOptions::new()
            .custom_flags(flags)
            .read(true)
            .write(true)
            .open(path)
            .map(|file| file.into())
            .map_err(|err| err.raw_os_error().unwrap_or(-1))
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

struct KeyboardState {
    ctrl_pressed: bool,
    last_device_name: String,
}

impl KeyboardState {
    fn new() -> Self {
        Self {
            ctrl_pressed: false,
            last_device_name: String::new(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("(fswitcher) Starting up...");

    let mut input = Libinput::new_with_udev(Interface);
    input.udev_assign_seat("seat0").unwrap();

    let input_fd = AsyncFd::new(input.as_raw_fd())?;

    let conn = Builder::address(
        "unix:path=/run/user/1000/at-spi/bus,guid=562a3d8fe328266fef2aa97769175f53",
    )?
    .build()
    .await?;

    // Get the AT-SPI registry proxy and register for events
    let registry = RegistryProxy::new(&conn).await?;
    println!("(fswitcher) Registering for AT-SPI events...");
    // Register for multiple event types to see what's available
    registry.register_event("object").await?;
    registry.register_event("focus").await?;
    registry.register_event("window").await?;
    println!("(fswitcher) Registered for AT-SPI events");

    // Subscribe to AT-SPI events - listen for Object and Window events
    let match_rule_object = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.a11y.atspi.Event.Object")?
        .build();

    let match_rule_window = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.a11y.atspi.Event.Window")?
        .build();

    let match_rule_focus = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.a11y.atspi.Event.Focus")?
        .build();

    let mut stream_object = MessageStream::for_match_rule(match_rule_object, &conn, None).await?;
    let mut stream_window = MessageStream::for_match_rule(match_rule_window, &conn, None).await?;
    let mut stream_focus = MessageStream::for_match_rule(match_rule_focus, &conn, None).await?;

    println!("(fswitcher) Listening for focus changes...");

    let mut keyboard_states: HashMap<u32, KeyboardState> = HashMap::new();

    let mut b_bindings: HashMap<u32, Option<String>> = HashMap::from([
        (1, None),    // built-in keyboard
        (8195, None), // external keyboard
    ]);

    loop {
        tokio::select! {
            guard = input_fd.readable() => {
                let mut guard = guard?;
                guard.clear_ready();

                input.dispatch()?;
                for event in &mut input {
                    handle_keyboard_event(event, &mut keyboard_states);
                }
            }

            Some(msg) = stream_object.next() => {
                if let Ok(_msg) = msg {
                    // handle_dbus_message(&_msg);
                }
            }

            Some(msg) = stream_window.next() => {
               if let Ok(_msg) = msg {
                   handle_dbus_message(&_msg, &mut b_bindings);
               }
            }

            Some(msg) = stream_focus.next() => {
                if let Ok(_msg) = msg {
                    handle_dbus_message(&_msg, &mut b_bindings);
                }
            }
        }
    }
}

fn handle_keyboard_event(event: LibinputEvent, states: &mut HashMap<u32, KeyboardState>) {
    if let LibinputEvent::Keyboard(keyboard_event) = event {
        let device = keyboard_event.device();
        let device_id = device.id_product();
        let key = keyboard_event.key();
        let is_ctrl = key == Key::LeftCtrl.key() || key == Key::RightCtrl.key();

        let state = states.entry(device_id).or_insert_with(KeyboardState::new);

        match keyboard_event.key_state() {
            input::event::keyboard::KeyState::Pressed => {
                if is_ctrl && !state.ctrl_pressed {
                    state.ctrl_pressed = true;
                    state.last_device_name = device.name().to_string();

                    println!(
                        "(fswitcher) Ctrl pressed on '{}' (vendor: {}, product: {})",
                        device.name(),
                        device.id_vendor(),
                        device.id_product()
                    );
                }
            }
            input::event::keyboard::KeyState::Released => {
                if is_ctrl && state.ctrl_pressed {
                    state.ctrl_pressed = false;
                    println!("(fswitcher) Ctrl released on '{}'", state.last_device_name);
                }
            }
        }
    }
}

fn handle_dbus_message(msg: &Message, b_bindings: &mut HashMap<u32, Option<String>>) {
    println!(
        "(fswitcher) D-Bus(a) Event: {:?} at{:?}",
        msg.header().member().unwrap().as_str(),
        msg.header().path().unwrap().as_str()
    );
    if let (Some(member), Some(interface), Some(path)) = (
        msg.header().member(),
        msg.header().interface(),
        msg.header().path(),
    ) {
        // Check for *either* the Focus event or the Window Activate event
        let is_focus_event = (interface.as_str() == "org.a11y.atspi.Event.Focus"
            && member.as_str() == "Focis")
            || (interface.as_str() == "org.a11y.atspi.Event.Window"
                && member.as_str() == "Activate")
            || (interface.as_str() == "org.a11y.atspi.Event.Window"
                && member.as_str() == "activate");

        if is_focus_event {
            println!("(fswitcher) D-Bus(a) signal: {}.{}", interface, member);

            // Your existing "push-down queue" logic
            if b_bindings.get(&1).is_some() && b_bindings.get(&8195).is_some() {
                b_bindings.insert(8195, b_bindings.get(&1).unwrap().clone());
                b_bindings.insert(1, Some(path.to_string()));
            } else if b_bindings.get(&1).is_none() && b_bindings.get(&8195).is_none() {
                b_bindings.insert(8195, Some(path.to_string()));
            } else {
                b_bindings.insert(1, Some(path.to_string()));
            }
            println!(
                "(fswitcher) Bindings at {}:\n\t1: {}\n\t8195: {}",
                Local::now().format("%H:%M:%S"),
                b_bindings
                    .get(&1)
                    .and_then(|v| v.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "None".into()),
                b_bindings
                    .get(&8195)
                    .and_then(|v| v.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "None".into())
            );
        }
        // Other events (like Deactivate) are now silently ignored
    }
}
