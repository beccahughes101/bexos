pub const IPC_PAYLOAD_SIZE: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endpoint {
    A,
    B,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    pub object_id: u64,
    pub rights: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    pub bytes: [u8; IPC_PAYLOAD_SIZE],
    pub len: usize,
    pub capability: Option<Capability>,
}

impl Message {
    pub fn new(payload: &[u8], capability: Option<Capability>) -> Result<Self, IpcError> {
        if payload.len() > IPC_PAYLOAD_SIZE {
            return Err(IpcError::PayloadTooLarge);
        }

        let mut bytes = [0; IPC_PAYLOAD_SIZE];
        let mut index = 0;
        while index < payload.len() {
            bytes[index] = payload[index];
            index += 1;
        }

        Ok(Self {
            bytes,
            len: payload.len(),
            capability,
        })
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcError {
    Full,
    Empty,
    PayloadTooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Channel<const N: usize> {
    to_a: Queue<N>,
    to_b: Queue<N>,
}

impl<const N: usize> Channel<N> {
    pub const fn new() -> Self {
        Self {
            to_a: Queue::new(),
            to_b: Queue::new(),
        }
    }

    pub fn send(&mut self, from: Endpoint, message: Message) -> Result<(), IpcError> {
        match from {
            Endpoint::A => self.to_b.push(message),
            Endpoint::B => self.to_a.push(message),
        }
    }

    pub fn receive(&mut self, endpoint: Endpoint) -> Result<Message, IpcError> {
        match endpoint {
            Endpoint::A => self.to_a.pop(),
            Endpoint::B => self.to_b.pop(),
        }
    }
}

impl<const N: usize> Default for Channel<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Queue<const N: usize> {
    entries: [Option<Message>; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<const N: usize> Queue<N> {
    const fn new() -> Self {
        Self {
            entries: [None; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    fn push(&mut self, message: Message) -> Result<(), IpcError> {
        if self.len == N {
            return Err(IpcError::Full);
        }

        self.entries[self.tail] = Some(message);
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Result<Message, IpcError> {
        if self.len == 0 {
            return Err(IpcError::Empty);
        }

        let message = self.entries[self.head].take().ok_or(IpcError::Empty)?;
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Ok(message)
    }
}
