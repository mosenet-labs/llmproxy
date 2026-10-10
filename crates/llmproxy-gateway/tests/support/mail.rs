//! Loopback SMTP fixture; never contacts a real mail service.
use super::Mock;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    io::{BufRead, BufReader, Write},
    sync::mpsc::{self, Receiver},
};

pub fn smtp() -> (Mock, Receiver<String>) {
    let (sender, receiver) = mpsc::channel();
    let server = Mock::raw(move |mut stream| {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        if stream.write_all(b"220 localhost test SMTP\r\n").is_err() {
            return;
        }
        loop {
            let mut line = String::new();
            if !reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                return;
            }
            let reply = if line.starts_with("EHLO") || line.starts_with("HELO") {
                "250-localhost\r\n250 8BITMIME\r\n"
            } else if line.starts_with("DATA") {
                if stream.write_all(b"354 Send message\r\n").is_err() {
                    return;
                }
                let mut message = String::new();
                loop {
                    let mut line = String::new();
                    if !reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                        return;
                    }
                    if line == ".\r\n" {
                        break;
                    }
                    message.push_str(&line);
                }
                let _ = sender.send(message);
                "250 accepted\r\n"
            } else if line.starts_with("QUIT") {
                let _ = stream.write_all(b"221 bye\r\n");
                return;
            } else {
                "250 OK\r\n"
            };
            if stream.write_all(reply.as_bytes()).is_err() {
                return;
            }
        }
    });
    (server, receiver)
}

pub fn code(message: &str) -> String {
    let (headers, body) = message.split_once("\r\n\r\n").unwrap();
    let body = if headers
        .to_lowercase()
        .contains("content-transfer-encoding: base64")
    {
        String::from_utf8(
            STANDARD
                .decode(body.split_whitespace().collect::<String>())
                .unwrap(),
        )
        .unwrap()
    } else if headers
        .to_lowercase()
        .contains("content-transfer-encoding: quoted-printable")
    {
        let compact = body.replace("=\r\n", "");
        let mut bytes = vec![];
        let mut offset = 0;
        while offset < compact.len() {
            if compact.as_bytes()[offset] == b'=' && offset + 2 < compact.len() {
                bytes.push(u8::from_str_radix(&compact[offset + 1..offset + 3], 16).unwrap());
                offset += 3;
            } else {
                bytes.push(compact.as_bytes()[offset]);
                offset += 1;
            }
        }
        String::from_utf8(bytes).unwrap()
    } else {
        body.into()
    };
    body.as_bytes()
        .windows(6)
        .find(|bytes| bytes.iter().all(u8::is_ascii_digit))
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .expect("six-digit email code")
}
