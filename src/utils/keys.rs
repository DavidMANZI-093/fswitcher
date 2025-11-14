#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
pub enum Key {
    LeftCtrl = 29,
    RightCtrl = 97,
}

impl Key {
    pub fn key(self) -> u32 {
        self as u32
    }
}
