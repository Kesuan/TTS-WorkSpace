//! 测试辅助：本地 HTTP mock（离线回放固定响应；测试禁止访问真实 API）。

#![allow(dead_code)] // 各测试目标按需使用

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct MockResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl MockResponse {
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: body.into(),
        }
    }

    pub fn sse(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream",
            body: body.into(),
        }
    }

    pub fn error(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.into(),
        }
    }
}

pub struct MockServer {
    /// 形如 `http://127.0.0.1:PORT/v1`。
    pub base_url: String,
    requests: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<String>>>,
    _handle: thread::JoinHandle<()>,
}

impl MockServer {
    /// 顺序返回给定响应；请求数超过列表长度时重复最后一个。
    pub fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 mock 端口失败");
        let port = listener.local_addr().expect("读取端口失败").port();
        let requests = Arc::new(AtomicUsize::new(0));
        let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::clone(&requests);
        let captured = Arc::clone(&bodies);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Some(body) = read_request(&mut stream) else {
                    continue;
                };
                captured.lock().expect("锁失败").push(body);
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let response = responses.get(index).or_else(|| responses.last());
                let Some(response) = response else { continue };
                let _ = write_response(&mut stream, response);
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            requests,
            bodies,
            _handle: handle,
        }
    }

    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// 捕获到的请求体（JSON 字符串，按到达顺序）。
    pub fn request_bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("锁失败").clone()
    }
}

/// 读取一个请求，返回请求体（读取失败返回 `None`）。
fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 4096];
    loop {
        match stream.read(&mut temp) {
            Ok(0) => return None,
            Ok(n) => buffer.extend_from_slice(&temp[..n]),
            Err(_) => return None,
        }
        if let Some(header_end) = find_header_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if buffer.len() - header_end >= content_length {
                return Some(
                    String::from_utf8_lossy(&buffer[header_end..header_end + content_length])
                        .to_string(),
                );
            }
        }
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn write_response(stream: &mut TcpStream, response: &MockResponse) -> std::io::Result<()> {
    let body = response.body.as_bytes();
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        status_text(response.status),
        response.content_type,
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// 一秒钟的静音 WAV（24kHz 单声道 16-bit）。
pub fn tiny_wav() -> Vec<u8> {
    tts_core::audio::pcm16_to_wav(&vec![0u8; 4_800], 24_000).expect("生成测试 WAV 失败")
}

/// 指定时长的 440Hz 正弦波 WAV（用于需要真实音频内容的测试，如 trim/normalize/拼接）。
pub fn tone_wav(duration_ms: u32) -> Vec<u8> {
    let sample_rate = 24_000u32;
    let frames = sample_rate * duration_ms / 1000;
    let mut pcm = Vec::with_capacity(frames as usize * 2);
    for index in 0..frames {
        let t = index as f32 / sample_rate as f32;
        let value = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
        let sample = (value * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    tts_core::audio::pcm16_to_wav(&pcm, sample_rate).expect("生成测试音频失败")
}

/// 非流式 chat.completions 响应（音频为 base64 WAV）。
pub fn chat_response_with_wav(wav: &[u8]) -> String {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(wav);
    serde_json::json!({
        "model": "mimo-v2.5-tts",
        "choices": [{ "message": { "audio": { "data": data } } }]
    })
    .to_string()
}

/// 流式 SSE 响应：给定 PCM 分片按 MiMo 帧格式编码。
pub fn chat_sse_with_pcm(chunks: &[Vec<u8>]) -> String {
    use base64::Engine as _;
    let mut body = String::new();
    for pcm in chunks {
        let data = base64::engine::general_purpose::STANDARD.encode(pcm);
        let frame = serde_json::json!({
            "choices": [{ "delta": { "audio": { "data": data } } }]
        });
        body.push_str("data: ");
        body.push_str(&frame.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}
