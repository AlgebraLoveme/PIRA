use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, ExitStatus, Output};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub fn announcement(child: &mut Child) -> io::Result<String> {
    let stderr = child.stderr.take().expect("piped announcement stream");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stderr).read_line(&mut line).map(|_| line);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(result) => result,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("announcement: {error}"),
            ))
        }
    }
}

pub fn wait(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "owned test process did not exit",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn drain(stream: Option<impl Read + Send + 'static>) -> Receiver<io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream.map_or(Ok(0), |mut pipe| pipe.read_to_end(&mut bytes));
        let _ = tx.send(result.map(|_| bytes));
    });
    rx
}

fn received(rx: Receiver<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    rx.recv_timeout(Duration::from_secs(5)).map_err(|error| {
        io::Error::new(io::ErrorKind::TimedOut, format!("output drain: {error}"))
    })?
}

pub fn output(mut child: Child) -> io::Result<Output> {
    drop(child.stdin.take());
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait(&mut child)?;
    Ok(Output {
        status,
        stdout: received(stdout)?,
        stderr: received(stderr)?,
    })
}
