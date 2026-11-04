use std::{
    env, fs,
    io::{self, Read, Write},
    process::{self, Command, Stdio},
    thread,
    time::Duration,
};

fn required_arg(args: &mut impl Iterator<Item = String>, name: &str) -> String {
    args.next().unwrap_or_else(|| panic!("missing {name}"))
}

fn milliseconds(value: String) -> Duration {
    Duration::from_millis(value.parse().expect("milliseconds must be an integer"))
}

fn main() {
    let mut args = env::args().skip(1);
    let mode = required_arg(&mut args, "mode");
    match mode.as_str() {
        "echo-args" => {
            serde_json::to_writer(io::stdout(), &args.collect::<Vec<_>>()).unwrap();
        }
        "stdin-length" => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).unwrap();
            print!("{}", input.len());
        }
        "stdin-echo" => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).unwrap();
            io::stdout().write_all(&input).unwrap();
        }
        "environment" => {
            for name in args {
                println!("{name}={}", u8::from(env::var_os(&name).is_some()));
            }
        }
        "emit" => {
            let stdout_len: usize = required_arg(&mut args, "stdout length").parse().unwrap();
            let stderr_len: usize = required_arg(&mut args, "stderr length").parse().unwrap();
            io::stdout().write_all(&vec![b'O'; stdout_len]).unwrap();
            io::stderr().write_all(&vec![b'E'; stderr_len]).unwrap();
        }
        "invalid-utf8" => {
            io::stdout().write_all(&[0xff, 0x00, 0xfe]).unwrap();
            io::stderr().write_all(&[0x80, b'E']).unwrap();
        }
        "lines" => {
            println!("alpha");
            println!("beta");
            eprintln!("warning");
        }
        "long-line" => {
            let length: usize = required_arg(&mut args, "line length").parse().unwrap();
            io::stdout().write_all(&vec![b'L'; length]).unwrap();
            io::stdout().write_all(b"\n").unwrap();
        }
        "sleep" => thread::sleep(milliseconds(required_arg(&mut args, "sleep duration"))),
        "exit" => process::exit(required_arg(&mut args, "exit code").parse().unwrap()),
        "mark-sleep" => {
            let marker = required_arg(&mut args, "marker path");
            fs::write(marker, process::id().to_string()).unwrap();
            thread::sleep(milliseconds(required_arg(&mut args, "sleep duration")));
        }
        "spawn-tree" => {
            let pid_file = required_arg(&mut args, "pid file");
            let sleep_for = required_arg(&mut args, "sleep duration");
            thread::sleep(Duration::from_millis(150));
            let mut child = Command::new(env::current_exe().unwrap())
                .arg("child-sleep")
                .arg(&sleep_for)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            fs::write(pid_file, format!("{}\n{}\n", process::id(), child.id())).unwrap();
            thread::sleep(milliseconds(sleep_for));
            let _ = child.wait();
        }
        "spawn-inherited-pipes-and-exit" => {
            let pid_file = required_arg(&mut args, "pid file");
            let sleep_for = required_arg(&mut args, "sleep duration");
            thread::sleep(Duration::from_millis(150));
            #[expect(
                clippy::zombie_processes,
                reason = "fixture must exit before its descendant; the supervisor test owns tree termination"
            )]
            let child = Command::new(env::current_exe().unwrap())
                .arg("child-sleep")
                .arg(sleep_for)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            fs::write(pid_file, format!("{}\n{}\n", process::id(), child.id())).unwrap();
        }
        "child-sleep" => {
            thread::sleep(milliseconds(required_arg(&mut args, "sleep duration")));
        }
        _ => panic!("unknown fixture mode"),
    }
}
