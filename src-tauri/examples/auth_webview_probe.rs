//! 验证 macOS/Linux 真实 WebView 引擎会在跨站 iframe 中发送认证 cookie。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

fn read_request(stream: &mut TcpStream) -> String {
    let mut buffer = [0_u8; 4096];
    let size = stream.read(&mut buffer).expect("read request");
    String::from_utf8_lossy(&buffer[..size]).into_owned()
}

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write response");
}

fn request_status(port: u16, path: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("negative control connect");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("negative control request");
    let response = read_request(&mut stream);
    response
        .split_whitespace()
        .nth(1)
        .expect("response status")
        .parse()
        .expect("numeric response status")
}

fn main() -> wry::Result<()> {
    let relay = TcpListener::bind("127.0.0.1:0").expect("bind relay");
    let core = TcpListener::bind("127.0.0.1:0").expect("bind core");
    let relay_port = relay.local_addr().expect("relay address").port();
    let core_port = core.local_addr().expect("core address").port();
    let authenticated = Arc::new(AtomicBool::new(false));

    let core_authenticated = Arc::clone(&authenticated);
    std::thread::spawn(move || {
        for incoming in core.incoming() {
            let mut stream = incoming.expect("core request");
            let request = read_request(&mut stream);
            let has_cookie = request.lines().any(|line| {
                line.to_ascii_lowercase()
                    .starts_with("cookie: dsh-auth-probe=signed")
            });
            if request.starts_with("GET /?token=one-shot ") {
                respond(
                    &mut stream,
                    "303 See Other",
                    "Location: /\r\nSet-Cookie: dsh-auth-probe=signed; Path=/; HttpOnly; SameSite=Strict\r\nCache-Control: no-store\r\n",
                    "",
                );
            } else if request.starts_with("POST /api/settings/describe ") {
                if has_cookie {
                    respond(
                        &mut stream,
                        "200 OK",
                        "Content-Type: application/json\r\n",
                        "{}",
                    );
                    core_authenticated.store(true, Ordering::SeqCst);
                } else {
                    respond(&mut stream, "401 Unauthorized", "", "unauthorized");
                }
            } else if has_cookie {
                let body = "<script>fetch('/api/settings/describe',{method:'POST',credentials:'include',headers:{'content-type':'application/json'},body:JSON.stringify({type:'client-request',rpcId:'probe',method:'settings/describe',payload:{args:{}}})});</script>";
                respond(&mut stream, "200 OK", "Content-Type: text/html\r\n", body);
            } else {
                respond(&mut stream, "401 Unauthorized", "", "unauthorized");
            }
            if core_authenticated.load(Ordering::SeqCst) {
                break;
            }
        }
    });

    assert_eq!(request_status(core_port, "/api/settings/describe"), 401);

    std::thread::spawn(move || {
        let (mut stream, _) = relay.accept().expect("relay request");
        let request = read_request(&mut stream);
        assert!(request.starts_with("GET /auth "));
        respond(
            &mut stream,
            "303 See Other",
            &format!(
                "Location: http://localhost:{core_port}/?token=one-shot\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n"
            ),
            "",
        );
    });

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_visible(false)
        .build(&event_loop)
        .expect("build window");
    let builder = WebViewBuilder::new()
        .with_custom_protocol("tauri".into(), move |_webview_id, _request| {
            wry::http::Response::builder()
                .header(wry::http::header::CONTENT_TYPE, "text/html")
                .body(
                    format!(
                        "<!doctype html><iframe src=\"http://localhost:{relay_port}/auth\"></iframe>"
                    )
                    .into_bytes(),
                )
                .expect("custom protocol response")
                .map(Into::into)
        })
        .with_url("tauri://localhost");
    #[cfg(target_os = "macos")]
    let _webview = builder.build(&window)?;
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().expect("GTK vbox"))?
    };

    let deadline = Instant::now() + Duration::from_secs(20);
    event_loop.run(move |_, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50));
        if authenticated.load(Ordering::SeqCst) {
            std::process::exit(0);
        }
        if Instant::now() >= deadline {
            eprintln!("protected iframe API did not receive the WebView cookie");
            std::process::exit(1);
        }
    });
}
