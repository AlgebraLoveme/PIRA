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
    let mut listener = Listener::bind(&root).unwrap();
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
        deadline: Some(Instant::now() + BUDGET),
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

#[test]
#[cfg(windows)]
fn windows_busy_pipe_reports_native_admission_error() {
    let mut listener = Listener::bind(Path::new(".")).unwrap();
    let endpoint = listener.endpoint();
    let _first = connect(&endpoint, Instant::now() + BUDGET).unwrap();
    // The first native instance is occupied until accept publishes another.
    // Hold that state deterministically; this is queue admission, not peer I/O.
    let error = connect(&endpoint, Instant::now() + Duration::from_millis(100))
        .err().expect("busy instance must not admit a second connection");
    eprintln!("named-pipe busy admission: kind={:?}, OS {:?}", error.kind(), error.raw_os_error());
    assert!(error.raw_os_error() == Some(121) || error.kind() == io::ErrorKind::TimedOut);
    let _accepted = listener.accept(Instant::now() + BUDGET).unwrap();
    let _second = connect(&endpoint, Instant::now() + BUDGET).unwrap();
}

#[test]
#[cfg(windows)]
fn windows_disconnected_unaccepted_peer_preserves_listener() {
    let mut listener = Listener::bind(Path::new(".")).unwrap();
    let endpoint = listener.endpoint();
    // A caller can be cancelled after kernel connection but before acceptance.
    drop(connect(&endpoint, Instant::now() + BUDGET).unwrap());
    let error = listener.accept(Instant::now() + Duration::from_millis(100))
        .err().expect("no live peer remains");
    eprintln!("unaccepted peer disconnect: kind={:?}, OS {:?}", error.kind(), error.raw_os_error());
    assert_eq!(error.kind(), io::ErrorKind::TimedOut,
        "an abandoned peer must not turn into a fatal listener error");
    let _client = connect(&endpoint, Instant::now() + BUDGET).unwrap();
    let _accepted = listener.accept(Instant::now() + BUDGET).unwrap();
}

#[test]
#[cfg(windows)]
fn windows_abandoned_peer_does_not_close_authentication_server() {
    let listener = Listener::bind(Path::new(".")).unwrap();
    let endpoint = listener.endpoint();
    drop(connect(&endpoint, Instant::now() + BUDGET).unwrap());
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let server = std::thread::spawn(move || {
        serve(listener, &stop, &Secret::new(b"synthetic response".to_vec()).unwrap(), 2,
            |peer, _| {
                assert_eq!(peer, std::process::id());
                Ok(())
            });
    });
    let received = (|| -> io::Result<Vec<u8>> {
        let mut client = connect(&endpoint, Instant::now() + BUDGET)?;
        if client.peer()? != std::process::id() {
            return Err(io::Error::other("unexpected synthetic server identity"));
        }
        write_frame(&mut client, b"{}")?;
        let mut status = [0];
        client.read_exact(&mut status)?;
        if status != [1] {
            return Err(io::Error::other("synthetic handoff rejected"));
        }
        let payload = read_frame(&mut client, 128)?;
        client.write_all(&[1])?;
        Ok(payload)
    })();
    stopped.store(true, Ordering::SeqCst);
    server.join().unwrap();
    assert_eq!(received.unwrap(), b"synthetic response");
}
