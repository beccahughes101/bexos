use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};
pub struct Qmp {
    read: BufReader<UnixStream>,
    write: UnixStream,
    directory: PathBuf,
}
pub fn pixel(image: &[u8], x: usize, y: usize) -> Option<[u8; 3]> {
    let start = image.len().checked_sub(800 * 600 * 3)?;
    image
        .get(start + (y * 800 + x) * 3..start + (y * 800 + x) * 3 + 3)?
        .try_into()
        .ok()
}
impl Qmp {
    pub fn connect(path: &Path, directory: PathBuf) -> Result<Self, String> {
        let stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .map_err(|e| e.to_string())?;
        let mut q = Self {
            read: BufReader::new(stream.try_clone().map_err(|e| e.to_string())?),
            write: stream,
            directory,
        };
        q.read()?;
        q.call("qmp_capabilities", json!({}))?;
        Ok(q)
    }
    fn read(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        self.read.read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }
    fn call(&mut self, command: &str, args: Value) -> Result<Value, String> {
        writeln!(
            self.write,
            "{}",
            json!({"execute":command,"arguments":args})
        )
        .map_err(|e| e.to_string())?;
        loop {
            let v = self.read()?;
            if let Some(e) = v.get("error") {
                return Err(e.to_string());
            }
            if let Some(r) = v.get("return") {
                return Ok(r.clone());
            }
        }
    }
    pub fn key(&mut self, key: &str) -> Result<(), String> {
        for down in [true, false] {
            self.call("input-send-event",json!({"events":[{"type":"key","data":{"down":down,"key":{"type":"qcode","data":key}}}]}))?;
            std::thread::sleep(Duration::from_millis(40));
        }
        Ok(())
    }
    pub fn type_text(&mut self, value: &str) -> Result<(), String> {
        for c in value.chars() {
            self.key(&c.to_string())?;
        }
        Ok(())
    }
    pub fn pointer(&mut self, x: u32, y: u32, down: Option<bool>) -> Result<(), String> {
        let mut events = vec![
            json!({"type":"abs","data":{"axis":"x","value":x*32767/800}}),
            json!({"type":"abs","data":{"axis":"y","value":y*32767/600}}),
        ];
        if let Some(down) = down {
            events.push(json!({"type":"btn","data":{"down":down,"button":"left"}}));
        }
        self.call("input-send-event", json!({"events":events}))?;
        std::thread::sleep(Duration::from_millis(100));
        Ok(())
    }
    pub fn click(&mut self, x: u32, y: u32) -> Result<(), String> {
        self.pointer(x, y, Some(true))?;
        self.pointer(x, y, Some(false))
    }
    pub fn wheel_down(&mut self) -> Result<(), String> {
        for down in [true, false] {
            self.call("input-send-event", json!({"events":[{"type":"btn","data":{"down":down,"button":"wheel-down"}}]}))?;
        }
        Ok(())
    }
    pub fn screenshot(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let path = self.directory.join(format!("{name}.ppm"));
        self.call("screendump", json!({"filename":path}))?;
        std::fs::read(path).map_err(|e| e.to_string())
    }
}
