use super::*;
use std::io::Write;

fn info(port: u16) -> String {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    line
}

#[test]
fn starts_with_jetstream_and_cleans_up() {
    let server = Server::start().unwrap();
    assert!(server.url().starts_with("nats://127.0.0.1:"));
    let line = info(server.port);
    assert!(line.contains("\"jetstream\":true"), "{line}");
    let dir = server.owned.as_ref().unwrap().dir.clone();
    assert!(dir.join("js").exists());
    drop(server);
    assert!(!dir.exists());
}

#[test]
fn extra_config_is_applied() {
    let server = Server::start_with(&Options {
        config: "authorization { users = [ { user: a, password: b } ] }".into(),
    })
    .unwrap();
    assert!(info(server.port).contains("\"auth_required\":true"));
}

#[test]
fn a_client_can_publish_and_subscribe() {
    let server = Server::start().unwrap();
    let sub = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let mut sub_reader = BufReader::new(sub.try_clone().unwrap());
    let mut line = String::new();
    sub_reader.read_line(&mut line).unwrap();
    (&sub)
        .write_all(b"CONNECT {\"verbose\":false}\r\nSUB demo.x 1\r\nPING\r\n")
        .unwrap();
    line.clear();
    sub_reader.read_line(&mut line).unwrap();
    assert_eq!(line, "PONG\r\n");
    let publisher = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    (&publisher)
        .write_all(b"CONNECT {\"verbose\":false}\r\nPUB demo.x 2\r\nhi\r\nPING\r\n")
        .unwrap();
    line.clear();
    sub_reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("MSG demo.x 1 2"), "{line}");
}

#[test]
fn test_prefixes_are_unique_and_follow_the_environment() {
    let server = Server::start().unwrap();
    // SAFETY of the test: nextest runs each test in its own process.
    unsafe { env::set_var(URL_VAR, server.url()) };
    let a = test_nats().unwrap();
    let b = test_nats().unwrap();
    unsafe { env::remove_var(URL_VAR) };
    assert_eq!(a.url(), server.url());
    assert_ne!(a.prefix(), b.prefix());
    assert_eq!(a.subject("events"), format!("{}.events", a.prefix()));
    assert_eq!(
        a.name("confirmed"),
        format!("{}_CONFIRMED", a.prefix()).to_ascii_uppercase()
    );
}
