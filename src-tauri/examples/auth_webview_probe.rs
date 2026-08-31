//! 验证 macOS/Linux 真实 WebView 顶层导航可完成官方认证交换。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    loop {
        let mut buffer = [0_u8; 1024];
        match stream.read(&mut buffer) {
            Ok(0) if request.is_empty() => return Ok(None),
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before HTTP headers completed",
                ));
            }
            Ok(size) => {
                request.extend_from_slice(&buffer[..size]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    return Ok(Some(String::from_utf8_lossy(&request).into_owned()));
                }
                if request.len() >= 4096 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "HTTP request headers exceed fixture limit",
                    ));
                }
            }
            Err(error)
                if request.is_empty()
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                    ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

fn request_status(port: u16, path: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("negative control connect");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("negative control request");
    let response = read_request(&mut stream)
        .expect("read negative control response")
        .expect("negative control response headers");
    response
        .split_whitespace()
        .nth(1)
        .expect("response status")
        .parse()
        .expect("numeric response status")
}

fn spawn_ipv6_competitor(
    port: u16,
    hit: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    server_error: Arc<Mutex<Option<String>>>,
) {
    let listener = TcpListener::bind(("::1", port)).expect("bind competing IPv6 listener");
    listener
        .set_nonblocking(true)
        .expect("set competing listener nonblocking");
    std::thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    hit.store(true, Ordering::SeqCst);
                    let _ = respond(&mut stream, "418 I'm a teapot", "", "");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    *server_error.lock().expect("competitor error lock") =
                        Some(format!("competing IPv6 listener failed: {error}"));
                    return;
                }
            }
        }
    });
}

fn main() -> wry::Result<()> {
    let relay = TcpListener::bind("127.0.0.1:0").expect("bind relay");
    let core = TcpListener::bind("127.0.0.1:0").expect("bind core");
    let relay_port = relay.local_addr().expect("relay address").port();
    let core_port = core.local_addr().expect("core address").port();
    let authenticated = Arc::new(AtomicBool::new(false));
    let ipv6_hit = Arc::new(AtomicBool::new(false));
    let server_error = Arc::new(Mutex::new(None::<String>));
    spawn_ipv6_competitor(
        relay_port,
        Arc::clone(&ipv6_hit),
        Arc::clone(&authenticated),
        Arc::clone(&server_error),
    );
    spawn_ipv6_competitor(
        core_port,
        Arc::clone(&ipv6_hit),
        Arc::clone(&authenticated),
        Arc::clone(&server_error),
    );

    let core_authenticated = Arc::clone(&authenticated);
    let core_error = Arc::clone(&server_error);
    std::thread::spawn(move || {
        for incoming in core.incoming() {
            let mut stream = match incoming {
                Ok(stream) => stream,
                Err(error) => {
                    *core_error.lock().expect("core error lock") =
                        Some(format!("core accept failed: {error}"));
                    break;
                }
            };
            let request = match read_request(&mut stream) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    *core_error.lock().expect("core error lock") =
                        Some(format!("core request failed: {error}"));
                    break;
                }
            };
            let has_cookie = request.lines().any(|line| {
                line.to_ascii_lowercase()
                    .starts_with("cookie: dsh-auth-probe=signed")
            });
            let (status, headers, body, marks_authenticated) = if request
                .starts_with("GET /?token=one-shot ")
            {
                (
                        "303 See Other",
                        "Location: /\r\nSet-Cookie: dsh-auth-probe=signed; Path=/; HttpOnly; SameSite=Strict\r\nCache-Control: no-store\r\n",
                        "",
                        false,
                    )
            } else if request.starts_with("POST /api/settings/describe ") {
                if has_cookie {
                    ("200 OK", "Content-Type: application/json\r\n", "{}", true)
                } else {
                    ("401 Unauthorized", "", "unauthorized", false)
                }
            } else if has_cookie {
                let body = "<script>fetch('/api/settings/describe',{method:'POST',credentials:'include',headers:{'content-type':'application/json'},body:JSON.stringify({type:'client-request',rpcId:'probe',method:'settings/describe',payload:{args:{}}})});</script>";
                ("200 OK", "Content-Type: text/html\r\n", body, false)
            } else {
                ("401 Unauthorized", "", "unauthorized", false)
            };
            if let Err(error) = respond(&mut stream, status, headers, body) {
                *core_error.lock().expect("core error lock") =
                    Some(format!("core response failed: {error}"));
                break;
            }
            if marks_authenticated {
                core_authenticated.store(true, Ordering::SeqCst);
            }
            if core_authenticated.load(Ordering::SeqCst) {
                break;
            }
        }
    });

    assert_eq!(request_status(core_port, "/api/settings/describe"), 401);

    let relay_error = Arc::clone(&server_error);
    std::thread::spawn(move || {
        for incoming in relay.incoming() {
            let mut stream = match incoming {
                Ok(stream) => stream,
                Err(error) => {
                    *relay_error.lock().expect("relay error lock") =
                        Some(format!("relay accept failed: {error}"));
                    return;
                }
            };
            let request = match read_request(&mut stream) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    *relay_error.lock().expect("relay error lock") =
                        Some(format!("relay request failed: {error}"));
                    return;
                }
            };
            if !request.starts_with("GET /auth ") {
                *relay_error.lock().expect("relay error lock") =
                    Some("relay received an unexpected HTTP request".into());
                return;
            }
            if let Err(error) = respond(
                &mut stream,
                "303 See Other",
                &format!(
                    "Location: http://127.0.0.1:{core_port}/?token=one-shot\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n"
                ),
                "",
            ) {
                *relay_error.lock().expect("relay error lock") =
                    Some(format!("relay response failed: {error}"));
            }
            return;
        }
    });

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_visible(false)
        .build(&event_loop)
        .expect("build window");
    let builder = WebViewBuilder::new().with_url(format!("http://127.0.0.1:{relay_port}/auth"));
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
        if let Some(error) = server_error.lock().expect("server error lock").take() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        if authenticated.load(Ordering::SeqCst) {
            assert!(
                !ipv6_hit.load(Ordering::SeqCst),
                "IPv6 competitor received an authentication navigation"
            );
            std::process::exit(0);
        }
        if Instant::now() >= deadline {
            eprintln!(
                "protected API did not authenticate; IPv6 competitor hit={}",
                ipv6_hit.load(Ordering::SeqCst)
            );
            std::process::exit(1);
        }
    });
}
