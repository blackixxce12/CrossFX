//! A `dbus-daemon` of the tests' own, for the D-Bus service ([`crate::dbus`]), the suspend
//! watcher ([`crate::sleep`]), the notifications ([`crate::notify`]) and the tray
//! ([`crate::tray`]). Never the session's bus, never the system's: a test that reached either
//! would be driving — or muting — the CrossFX the user is listening to.
//!
//! Started through `fxsound_core::test_support`, so it cannot outlive the test: it is killed
//! when dropped, which a panicking test does too, and when the test process dies, however it
//! dies.
//!
//! zbus 6 has no blocking API, and a test is a thread that blocks: [`Client`] is the tests' own
//! end of a bus, in its place.

use fxsound_core::test_support::{self as support, Guarded, ScratchDir};
use std::future::Future;
use std::io::{BufRead as _, BufReader};
use std::pin::Pin;
use std::process::{ChildStdout, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use zbus::export::futures_core::Stream;

/// A private bus, killed when dropped.
pub(crate) struct PrivateBus {
    daemon: Guarded,
    /// What a client connects to.
    pub(crate) address: String,
    /// Kept open: the daemon has nowhere to write otherwise.
    _stdout: BufReader<ChildStdout>,
    /// The bus's configuration and socket, made new for it and removed with it.
    _dir: ScratchDir,
}

impl PrivateBus {
    /// A bus with the session bus's rules and none of its service files. `None`, after saying
    /// so, where `dbus-daemon` is not installed.
    ///
    /// Not `dbus-daemon --session`: that configuration reads `/usr/share/dbus-1/services`, where a
    /// CrossFX package installs `io.github.blackixxce12.CrossFX.service`, and a call to the name
    /// after the test's own service let go of it would start the installed CrossFX — with this
    /// process's environment, on the session's PipeWire. Here a call nobody owns the name for just
    /// fails.
    pub(crate) fn start() -> Option<Self> {
        Self::typed("session")
    }

    /// A bus of the system's type, as logind speaks on — `<type>system</type>` — listening in a
    /// scratch directory rather than on `/run/dbus/system_bus_socket`, and with a policy that lets
    /// a test own `org.freedesktop.login1`, which the real system bus reserves for root. `None`,
    /// after saying so, where `dbus-daemon` is not installed.
    pub(crate) fn start_system_like() -> Option<Self> {
        Self::typed("system")
    }

    /// A bus of type `kind` in a scratch directory, anyone allowed to own any name, and no
    /// service directory: nothing is ever activated.
    fn typed(kind: &str) -> Option<Self> {
        let dir = ScratchDir::for_sockets("bus");
        let config = dir.join(format!("{kind}-like.conf"));
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>{kind}</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                dir.display()
            ),
        )
        .expect("write the bus configuration");
        let flag = format!("--config-file={}", config.display());
        Self::spawn(&[flag.as_str()], dir)
    }

    fn spawn(config: &[&str], dir: ScratchDir) -> Option<Self> {
        let mut daemon = support::command("dbus-daemon");
        daemon
            .args(config)
            .args(["--print-address=1", "--nofork"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let spawned = support::spawn(daemon);
        let mut daemon = match spawned {
            Ok(daemon) => daemon,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping: dbus-daemon is not installed");
                return None;
            }
            Err(err) => panic!("dbus-daemon did not start: {err}"),
        };
        let mut stdout = BufReader::new(daemon.take_stdout().expect("piped"));
        let mut address = String::new();
        stdout.read_line(&mut address).expect("the bus address");
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon printed no address");
        Some(Self {
            daemon,
            address,
            _stdout: stdout,
            _dir: dir,
        })
    }

    /// A connection to the bus, for a test to call or signal from.
    pub(crate) fn client(&self) -> Client {
        Client::build(
            zbus::connection::Builder::address(self.address.as_str())
                .method_timeout(Duration::from_secs(10)),
        )
        .expect("a client connection to the private bus")
    }

    /// Whether `name` is owned on the bus, asked through `client`.
    pub(crate) fn has_owner(&self, client: &Client, name: &str) -> bool {
        let _ = self;
        let proxy = client
            .block_on(zbus::fdo::DBusProxy::new(client.connection()))
            .expect("the bus's own proxy");
        client
            .block_on(proxy.name_has_owner(name.try_into().expect("a bus name")))
            .expect("NameHasOwner")
    }
}

impl Drop for PrivateBus {
    /// The daemon goes first, and its directory after it, with the fields.
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// A test's own connection to a bus, standing in for the blocking connection zbus 6 no longer
/// has.
///
/// The connection runs on a current-thread tokio runtime that a thread of the client's own drives
/// for as long as the client lives, so whatever reaches it — a call to an interface it serves, a
/// signal one of its streams subscribed to, a property change its proxies cache — is taken in as
/// it arrives, as zbus 5's executor thread took it, whatever the test thread is waiting on
/// meanwhile. The test thread waits for its own calls with [`Client::block_on`]: tokio lets one
/// thread wait on a current-thread runtime that another drives, and polls the waiting thread's
/// future on that thread while the driving one reads the socket.
///
/// Dropping it leaves the bus, as dropping zbus 5's blocking connection did once nothing else held
/// it: the connection is closed even while a proxy or a stream still holds a clone of it.
pub(crate) struct Client {
    /// `None` only once closed, on the way out.
    connection: Option<zbus::Connection>,
    runtime: Arc<Runtime>,
    /// Dropped to stop the driving thread.
    stop: Option<oneshot::Sender<()>>,
    driver: Option<JoinHandle<()>>,
}

impl Client {
    /// The connection `builder` makes, on a runtime and a thread of its own.
    pub(crate) fn build(builder: zbus::connection::Builder<'static>) -> zbus::Result<Self> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()?,
        );
        // Built inside the runtime, which is what puts the connection on it.
        let connection = runtime.block_on(builder.build())?;
        let (stop, stopped) = oneshot::channel::<()>();
        let driver = {
            let runtime = Arc::clone(&runtime);
            thread::Builder::new()
                .name("crossfx-test-bus".to_owned())
                .spawn(move || {
                    // Nothing is ever sent; the sender going is the whole message.
                    let _ = runtime.block_on(stopped);
                })?
        };
        Ok(Self {
            connection: Some(connection),
            runtime,
            stop: Some(stop),
            driver: Some(driver),
        })
    }

    /// The connection, for a proxy or a call.
    pub(crate) fn connection(&self) -> &zbus::Connection {
        self.connection.as_ref().expect("open until dropped")
    }

    /// Wait for `future` on the client's runtime.
    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Run `task` on the client's runtime, alongside whatever the test does meanwhile.
    pub(crate) fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        drop(self.runtime.spawn(task));
    }

    /// Leave the bus.
    pub(crate) fn close(mut self) -> zbus::Result<()> {
        match self.connection.take() {
            Some(connection) => self.block_on(connection.close()),
            None => Ok(()),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = self.runtime.block_on(connection.close());
        }
        self.stop = None;
        if let Some(driver) = self.driver.take() {
            let _ = driver.join();
        }
    }
}

/// The next item of `stream`, or `None` once it has ended: what `StreamExt::next` would be.
pub(crate) async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}
