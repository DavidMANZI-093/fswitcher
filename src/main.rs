use futures_util::{StreamExt};
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
use zbus::Message;
use zbus::MessageStream;
use zbus::connection::Builder;

mod utils;

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
    println!("--- fswitcher starting up ---");

    let mut input = Libinput::new_with_udev(Interface);
    input.udev_assign_seat("seat0").unwrap();

    let input_fd = AsyncFd::new(input.as_raw_fd())?;

    let conn = Builder::address(
        "unix:path=/run/user/1000/at-spi/bus,guid=94030f18bd4301760318e7de69138161",
    )?
    .build()
    .await?;

    // let rule = MatchRule::builder()
    //     // .msg_type(zbus::message::Type::Signal)
    //     .interface("org.a11y.atspi.Event.Object")?
    //     .build();

    let mut stream = MessageStream::from(&conn);

    println!("Listening for focus changes...");

    let mut keyboard_states: HashMap<u32, KeyboardState> = HashMap::new();

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

            Some(msg) = stream.next() => {
                println!("Received message {:?}", msg);
                if let Ok(msg) = msg {
                    handle_dbus_message(msg);
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
                        "Ctrl pressed on '{}' (vendor: {}, product: {})",
                        device.name(),
                        device.id_vendor(),
                        device.id_product()
                    );
                }
            }
            input::event::keyboard::KeyState::Released => {
                if is_ctrl && state.ctrl_pressed {
                    state.ctrl_pressed = false;
                    println!("Ctrl released on '{}'", state.last_device_name);
                }
            }
        }
    }
}

fn handle_dbus_message(msg: Message) {
    if let Some(member) = msg.header().member() {
        println!("D-Bus signal received: {}", member);
        // let items = msg.header();
        // for item in items {
        //     println!("  Arg: {:?}", item);
        // }
    }
}
