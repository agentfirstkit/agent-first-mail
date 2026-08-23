#![allow(clippy::expect_used, clippy::panic)]

use std::net::TcpListener;
use std::process::{Command, Output};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestContainerRuntime {
    Apple,
    Docker,
}

impl TestContainerRuntime {
    pub fn selected() -> Self {
        match std::env::var("AFMAIL_TEST_CONTAINER_RUNTIME").as_deref() {
            Ok("apple") => Self::Apple,
            Ok("docker") => Self::Docker,
            Ok(_) => panic!("AFMAIL_TEST_CONTAINER_RUNTIME must be apple or docker"),
            Err(_) if cfg!(target_os = "macos") => Self::Apple,
            Err(_) => Self::Docker,
        }
    }

    pub fn is_apple(self) -> bool {
        self == Self::Apple
    }

    fn bin(self) -> &'static str {
        match self {
            Self::Apple => "container",
            Self::Docker => "docker",
        }
    }

    pub fn success(self, args: &[&str], context: &str) {
        let output_result = Command::new(self.bin()).args(args).output();
        assert!(
            output_result.is_ok(),
            "{context} failed to start {}: {:?}",
            self.bin(),
            output_result.as_ref().err()
        );
        let output = match output_result {
            Ok(output) => output,
            Err(_) => return,
        };
        assert!(
            output.status.success(),
            "{context} failed under {}: {}{}",
            self.bin(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn output(self, args: &[&str], context: &str) -> Output {
        Command::new(self.bin())
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("{context} failed to start {}: {error}", self.bin()))
    }

    pub fn published_ports(self, container_ports: &[u16]) -> Vec<PublishedPort> {
        if self == Self::Docker {
            return container_ports
                .iter()
                .copied()
                .map(|container_port| PublishedPort {
                    container_port,
                    host_port: None,
                    spec: format!("127.0.0.1::{container_port}"),
                })
                .collect();
        }

        // Apple container requires an explicit host port. Reserve every port
        // together so one fixture cannot choose the same port twice, then
        // release them immediately before `container run` binds them.
        let listeners = container_ports
            .iter()
            .map(|_| TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback test port"))
            .collect::<Vec<_>>();
        let ports = container_ports
            .iter()
            .copied()
            .zip(listeners.iter())
            .map(|(container_port, listener)| {
                let host_port = listener
                    .local_addr()
                    .expect("read reserved loopback port")
                    .port();
                PublishedPort {
                    container_port,
                    host_port: Some(host_port),
                    spec: format!("127.0.0.1:{host_port}:{container_port}"),
                }
            })
            .collect();
        drop(listeners);
        ports
    }

    pub fn mapped_port(self, container: &str, binding: &PublishedPort) -> u16 {
        if let Some(host_port) = binding.host_port {
            return host_port;
        }
        let private_port = format!("{}/tcp", binding.container_port);
        let output = self.output(&["port", container, &private_port], "inspect mapped port");
        assert!(
            output.status.success(),
            "inspect mapped port failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let port_text = String::from_utf8_lossy(&output.stdout);
        port_text
            .trim()
            .rsplit(':')
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("could not parse Docker port output: {port_text}"))
    }

    pub fn delete(self, container: &str) {
        let args: &[&str] = match self {
            Self::Apple => &["delete", "--force", container],
            Self::Docker => &["rm", "-f", container],
        };
        let _ = Command::new(self.bin()).args(args).status();
    }
}

pub struct PublishedPort {
    container_port: u16,
    host_port: Option<u16>,
    pub spec: String,
}
