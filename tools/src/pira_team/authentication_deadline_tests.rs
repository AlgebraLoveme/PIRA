#[cfg(unix)]
fn deadline_fixture() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "team-auth-deadline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    crate::private_dir(&path).unwrap();
    path
}

#[test]
#[cfg(unix)]
fn unix_listener_accept_honors_deadline_without_peer() {
    let root = deadline_fixture();
    let listener = Listener::bind(&root).unwrap();
    let start = Instant::now();
    assert_eq!(
        listener
            .accept(start + Duration::from_millis(30))
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn unix_connect_rejects_expired_deadline_before_socket_setup() {
    assert_eq!(
        connect_name(b"unused\0", Instant::now())
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
#[cfg(unix)]
fn unix_connection_stop_interrupts_wait_without_peer_or_path() {
    let (reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let mut connection = Connection {
        stream: reader,
        deadline: Instant::now() + BUDGET,
        stopped: Some(stopped.clone()),
    };
    let start = Instant::now();
    let (entered, ready) = std::sync::mpsc::channel();
    let (done, result) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        entered.send(()).unwrap();
        done.send(connection.read(&mut [0])).unwrap();
    });
    ready.recv_timeout(BUDGET).unwrap();
    stopped.store(true, Ordering::SeqCst);
    let early = result.recv_timeout(Duration::from_secs(1));
    if early.is_err() {
        let _ = writer.write_all(&[0]);
        let _ = result.recv_timeout(BUDGET).unwrap();
    }
    worker.join().unwrap();
    assert_eq!(
        early.unwrap().unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
#[cfg(target_os = "macos")]
fn credential_server_drop_survives_moved_socket_without_orphan_thread() {
    let root = deadline_fixture();
    // Synthetic memory only: do not capture a caller authentication environment/file.
    let server = Server::listen(
        &root,
        &json!(1),
        Secret::new(b"fixture only".to_vec()).unwrap(),
    )
    .unwrap();
    let endpoint = server.endpoint();
    let original = root.join("credential.sock");
    let moved = root.join("moved.sock");
    std::fs::rename(&original, &moved).unwrap();
    std::fs::write(&original, b"replacement must survive").unwrap();
    let (done, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        drop(server);
        done.send(()).unwrap();
    });
    let bounded = receiver.recv_timeout(Duration::from_secs(1)).is_ok();
    if !bounded {
        // Regression fail-safe: restore this test's path and wake its own listener before asserting.
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&moved, &original).unwrap();
        let _wake = connect(&endpoint, Instant::now() + BUDGET).unwrap();
        receiver.recv_timeout(BUDGET).unwrap();
    }
    worker.join().unwrap();
    if bounded {
        assert_eq!(
            std::fs::read(&original).unwrap(),
            b"replacement must survive"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        bounded,
        "credential listener shutdown depended on its mutable pathname"
    );
}
