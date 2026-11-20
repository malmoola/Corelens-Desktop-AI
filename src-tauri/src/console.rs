//! Serial console sessions: a device's console port, over a USB console
//! cable or a COM port on this machine, in a terminal tab like any SSH one.
//!
//! A console is what is left when the network is not: a switch with no
//! address yet, a router in ROMMON, a firewall that has lost its management
//! interface. SecureCRT's Serial protocol is the model — a port, a speed,
//! 8N1, a tab — with one thing added: the port is found rather than guessed.
//! `console_ports` lists what the system has and puts the console cables
//! first, by the USB vendor of the adapter inside them, and the dialog picks
//! the one a cable was just plugged into.
//!
//! The session itself is `terminal::spawn_session`'s, so a console tab has
//! the log, Find, send-to-many and the rest exactly as an SSH tab does. No
//! credential is involved: a console asks for its own login on the line, and
//! what is typed there goes to the port as typed.
//!
//! The port is driven by one thread, because the operating system's serial
//! calls block: it reads with a short timeout, and between reads writes what
//! the window sent and sends a break when asked.
use std::io::Read as _;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortInfo, SerialPortType, StopBits};
use tauri::{AppHandle, State};
use tokio::sync::mpsc;

use crate::commands::AppState;

type CmdResult<T> = Result<T, String>;

/// How long a read waits before the thread looks for something to write.
/// Short enough that a keystroke is not noticeably late.
const POLL: Duration = Duration::from_millis(20);

/// A write that has made no progress for this long has stalled: hardware
/// flow control waiting on a line nothing drives, most likely.
const STALLED: Duration = Duration::from_secs(10);

/// The speeds offered, the way every terminal program offers them. 9600 is
/// what every vendor's console ships at.
pub const SPEEDS: &[u32] = &[1200, 2400, 4800, 9600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600];

/// USB vendors whose serial adapters are console cables: Cisco's own USB
/// console port, and the chips inside the blue cables and USB-to-RJ45 leads.
const CONSOLE_VENDORS: &[(u16, &str)] = &[
    (0x05A6, "Cisco USB console"),
    (0x0403, "FTDI"),
    (0x067B, "Prolific"),
    (0x10C4, "Silicon Labs CP210x"),
    (0x1A86, "WCH CH340"),
    (0x0557, "ATEN"),
];

/// One port, as the console dialog lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsolePort {
    /// What it is opened by: `COM3`, `/dev/ttyUSB0`.
    pub name: String,
    /// `usb`, `pci`, `bluetooth` or `other`.
    pub kind: &'static str,
    /// What the system says it is, for a person to recognise.
    pub description: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    /// A known console adapter. The first of these is the one chosen.
    pub console: bool,
}

/// The ports, best first: known console adapters, then any other USB
/// serial port, then built-in ones, then Bluetooth links — which are listed
/// as COM ports on Windows and hang whoever opens one with nothing paired.
pub fn rank(ports: Vec<SerialPortInfo>) -> Vec<ConsolePort> {
    let names: Vec<String> = ports.iter().map(|p| p.port_name.clone()).collect();
    let mut out: Vec<ConsolePort> = ports
        .into_iter()
        // macOS lists every adapter twice, as tty.* (waits for carrier) and
        // cu.* (does not). The cu.* one is the one to open.
        .filter(|p| match p.port_name.strip_prefix("/dev/tty.") {
            Some(rest) => !names.iter().any(|n| n == &format!("/dev/cu.{rest}")),
            None => true,
        })
        .map(|p| match p.port_type {
            SerialPortType::UsbPort(usb) => {
                let vendor = CONSOLE_VENDORS.iter().find(|(v, _)| *v == usb.vid).map(|(_, n)| *n);
                let said = usb
                    .product
                    .clone()
                    .or(usb.manufacturer.clone())
                    .filter(|s| !s.trim().is_empty());
                let description = match (said, vendor) {
                    (Some(s), _) => s,
                    (None, Some(v)) => format!("{v} serial adapter"),
                    (None, None) => "USB serial adapter".to_string(),
                };
                ConsolePort {
                    name: p.port_name,
                    kind: "usb",
                    description,
                    vid: Some(usb.vid),
                    pid: Some(usb.pid),
                    console: vendor.is_some(),
                }
            }
            SerialPortType::PciPort => ConsolePort { name: p.port_name, kind: "pci", description: "Serial card".into(), vid: None, pid: None, console: false },
            SerialPortType::BluetoothPort => ConsolePort { name: p.port_name, kind: "bluetooth", description: "Bluetooth serial link".into(), vid: None, pid: None, console: false },
            SerialPortType::Unknown => ConsolePort { name: p.port_name, kind: "other", description: "Built-in serial port".into(), vid: None, pid: None, console: false },
        })
        .collect();
    let place = |p: &ConsolePort| match (p.console, p.kind) {
        (true, _) => 0,
        (false, "usb") => 1,
        (false, "pci") | (false, "other") => 2,
        _ => 3,
    };
    out.sort_by(|a, b| place(a).cmp(&place(b)).then_with(|| natural(&a.name).cmp(&natural(&b.name))));
    out
}

/// `COM3` before `COM10`: the name with its trailing number read as one.
fn natural(name: &str) -> (String, u64) {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, number) = name.split_at(name.len() - digits);
    (stem.to_string(), number.parse().unwrap_or(0))
}

/// Refuses anything that is not the name of a serial port, so this command
/// can never be pointed at an arbitrary file.
pub fn check_port_name(name: &str) -> CmdResult<String> {
    let name = name.trim();
    let ok = if let Some(n) = name.strip_prefix(r"\\.\").or(Some(name)).and_then(|n| n.strip_prefix("COM")) {
        !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())
    } else if let Some(rest) = name.strip_prefix("/dev/") {
        let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-:".contains(c));
        match rest.split_once('/') {
            None => (rest.starts_with("tty") || rest.starts_with("cu.")) && plain(rest),
            Some(("pts", n)) => !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()),
            Some(("serial", by)) => by.split_once('/').is_some_and(|(kind, id)| (kind == "by-id" || kind == "by-path") && plain(id)),
            Some(_) => false,
        }
    } else {
        false
    };
    if ok && !name.contains("..") {
        Ok(name.to_string())
    } else {
        Err(format!("{name:?} is not a serial port."))
    }
}

/// What a console's line is set to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSettings {
    pub speed: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow: FlowControl,
}

impl LineSettings {
    /// From the dialog's words. Anything left out is 8N1 without flow control.
    pub fn from_words(speed: u32, data_bits: Option<u8>, parity: Option<&str>, stop_bits: Option<u8>, flow: Option<&str>) -> CmdResult<Self> {
        if !SPEEDS.contains(&speed) {
            return Err(format!("{speed} is not a speed a console runs at."));
        }
        Ok(Self {
            speed,
            data_bits: match data_bits.unwrap_or(8) {
                8 => DataBits::Eight,
                7 => DataBits::Seven,
                n => return Err(format!("{n} data bits is not something a console uses.")),
            },
            parity: match parity.unwrap_or("none") {
                "none" => Parity::None,
                "even" => Parity::Even,
                "odd" => Parity::Odd,
                p => return Err(format!("{p:?} is not a parity.")),
            },
            stop_bits: match stop_bits.unwrap_or(1) {
                1 => StopBits::One,
                2 => StopBits::Two,
                n => return Err(format!("{n} stop bits is not something a console uses.")),
            },
            flow: match flow.unwrap_or("none") {
                "none" => FlowControl::None,
                "hardware" => FlowControl::Hardware,
                "software" => FlowControl::Software,
                f => return Err(format!("{f:?} is not a kind of flow control.")),
            },
        })
    }
}

/// Opens a port as a console wants it.
pub fn open_port(name: &str, line: &LineSettings) -> CmdResult<Box<dyn SerialPort>> {
    serialport::new(name, line.speed)
        .data_bits(line.data_bits)
        .parity(line.parity)
        .stop_bits(line.stop_bits)
        .flow_control(line.flow)
        .timeout(POLL)
        .open()
        .map_err(|e| why_not_opened(name, &e))
}

/// An open failure as a person should read it.
fn why_not_opened(name: &str, e: &serialport::Error) -> String {
    let said = e.to_string();
    let lower = said.to_lowercase();
    if matches!(e.kind, serialport::ErrorKind::NoDevice) || lower.contains("cannot find") || lower.contains("no such file") {
        format!("{name} is not there any more. Is the cable still plugged in?")
    } else if lower.contains("access is denied") || lower.contains("busy") || lower.contains("in use") {
        format!("{name} is in use by another program — PuTTY, SecureCRT, or another console tab. Close it there first.")
    } else if lower.contains("permission denied") {
        format!("This account may not open {name}. On Linux, add it to the dialout group and sign in again.")
    } else {
        format!("{name} could not be opened: {said}")
    }
}

/// What the port's thread is asked to do.
enum ToPort {
    Write(Vec<u8>),
    Break,
}

/// What the port's thread reports.
enum FromPort {
    Data(Vec<u8>),
    Ended(String),
}

/// An open console port, as the session loop drives it.
pub struct SerialLink {
    to_port: std_mpsc::Sender<ToPort>,
    from_port: mpsc::Receiver<FromPort>,
    ended: Option<String>,
}

impl SerialLink {
    /// Starts the thread that owns the port. The thread ends when the link
    /// is dropped or the port stops answering.
    pub fn start(port: Box<dyn SerialPort>, name: String) -> Self {
        let (to_port, commands) = std_mpsc::channel::<ToPort>();
        let (reports, from_port) = mpsc::channel::<FromPort>(256);
        std::thread::Builder::new()
            .name(format!("console {name}"))
            .spawn(move || drive(port, &name, commands, reports))
            .expect("a thread for the console port");
        Self { to_port, from_port, ended: None }
    }

    /// The next bytes from the port, or `None` once it has ended.
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        match self.from_port.recv().await? {
            FromPort::Data(bytes) => Some(bytes),
            FromPort::Ended(why) => {
                self.ended = Some(why);
                None
            }
        }
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.to_port
            .send(ToPort::Write(bytes.to_vec()))
            .map_err(|_| self.why_ended())
    }

    pub fn send_break(&mut self) -> Result<(), String> {
        self.to_port.send(ToPort::Break).map_err(|_| self.why_ended())
    }

    /// Why the port went, as its thread said.
    pub fn why_ended(&self) -> String {
        self.ended.clone().unwrap_or_else(|| "The console port closed.".to_string())
    }

    /// Dropping the sender is what tells the thread to stop and let go of
    /// the port, within one poll.
    pub fn close(self) {}
}

/// The port's thread: read, and between reads, write and break.
fn drive(mut port: Box<dyn SerialPort>, name: &str, commands: std_mpsc::Receiver<ToPort>, reports: mpsc::Sender<FromPort>) {
    let mut buf = [0u8; 4096];
    let end = |why: String| {
        let _ = reports.blocking_send(FromPort::Ended(why));
    };
    loop {
        loop {
            match commands.try_recv() {
                Ok(ToPort::Write(bytes)) => {
                    if let Err(why) = write_patiently(port.as_mut(), &bytes, name) {
                        return end(why);
                    }
                }
                Ok(ToPort::Break) => {
                    // A quarter of a second of break is longer than any
                    // console needs and shorter than anyone notices.
                    if port.set_break().is_ok() {
                        std::thread::sleep(Duration::from_millis(250));
                        let _ = port.clear_break();
                    }
                }
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => return,
            }
        }
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                if reports.blocking_send(FromPort::Data(buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => {}
            Err(e) => return end(format!("{name} stopped answering — was the cable unplugged? ({e})")),
        }
    }
}

/// Writes everything, however slowly the line takes it: a paste at 9600
/// baud is a second per kilobyte, which is many write timeouts.
fn write_patiently(port: &mut dyn SerialPort, mut bytes: &[u8], name: &str) -> Result<(), String> {
    let mut last = Instant::now();
    while !bytes.is_empty() {
        match port.write(bytes) {
            Ok(0) => {}
            Ok(n) => {
                bytes = &bytes[n..];
                last = Instant::now();
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => {}
            Err(e) => return Err(format!("{name} stopped taking what was typed — was the cable unplugged? ({e})")),
        }
        if !bytes.is_empty() && last.elapsed() > STALLED {
            return Err(format!("{name} has taken nothing for ten seconds. If flow control is on, try it off."));
        }
    }
    let _ = port.flush();
    Ok(())
}

/// Every serial port this machine has, best first.
#[tauri::command]
pub fn console_ports() -> CmdResult<Vec<ConsolePort>> {
    serialport::available_ports()
        .map(rank)
        .map_err(|e| format!("The serial ports could not be listed: {e}"))
}

/// Opens a console session on a port and returns its id, which every
/// `ssh_*` session command then takes.
// Flat arguments, as every Tauri command's are.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn console_open(
    app: AppHandle,
    state: State<'_, AppState>,
    port: String,
    speed: u32,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<u8>,
    flow: Option<String>,
) -> CmdResult<String> {
    let name = check_port_name(&port)?;
    let line = LineSettings::from_words(speed, data_bits, parity.as_deref(), stop_bits, flow.as_deref())?;
    let opening = name.clone();
    let opened = tauri::async_runtime::spawn_blocking(move || open_port(&opening, &line))
        .await
        .map_err(|e| format!("{name} could not be opened: {e}"))??;
    let id = format!("console-{}", uuid::Uuid::new_v4());
    let link = crate::terminal::Link::Serial(SerialLink::start(opened, name.clone()));
    crate::terminal::spawn_session(&app, &state, id.clone(), name, link, None)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::UsbPortInfo;

    /// `9600 8N1`, to compare a line with in one string.
    fn short(line: &LineSettings) -> String {
        let bits = match line.data_bits {
            DataBits::Seven => 7,
            DataBits::Five => 5,
            DataBits::Six => 6,
            DataBits::Eight => 8,
        };
        let parity = match line.parity {
            Parity::None => 'N',
            Parity::Even => 'E',
            Parity::Odd => 'O',
        };
        let stop = match line.stop_bits {
            StopBits::One => 1,
            StopBits::Two => 2,
        };
        format!("{} {bits}{parity}{stop}", line.speed)
    }

    fn usb(name: &str, vid: u16, product: Option<&str>) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.into(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid,
                pid: 0x0001,
                serial_number: None,
                manufacturer: None,
                product: product.map(String::from),
            }),
        }
    }
    fn plain(name: &str, kind: SerialPortType) -> SerialPortInfo {
        SerialPortInfo { port_name: name.into(), port_type: kind }
    }

    #[test]
    fn a_console_cable_comes_before_every_other_port() {
        let ranked = rank(vec![
            plain("COM1", SerialPortType::Unknown),
            plain("COM4", SerialPortType::BluetoothPort),
            usb("COM7", 0x2341, Some("Some board")),
            usb("COM12", 0x05A6, None),
            usb("COM5", 0x067B, Some("USB-Serial Controller")),
            plain("COM2", SerialPortType::PciPort),
        ]);
        let names: Vec<&str> = ranked.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["COM5", "COM12", "COM7", "COM1", "COM2", "COM4"]);
        assert!(ranked[0].console && ranked[1].console && !ranked[2].console);
        assert_eq!(ranked[1].description, "Cisco USB console serial adapter");
        assert_eq!(ranked[0].description, "USB-Serial Controller");
        assert_eq!(ranked[5].kind, "bluetooth");
    }

    #[test]
    fn com_ten_sorts_after_com_three_and_macos_twins_are_listed_once() {
        let ranked = rank(vec![
            usb("COM10", 0x0403, None),
            usb("COM3", 0x0403, None),
            usb("/dev/tty.usbserial-A1", 0x0403, None),
            usb("/dev/cu.usbserial-A1", 0x0403, None),
        ]);
        let names: Vec<&str> = ranked.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["/dev/cu.usbserial-A1", "COM3", "COM10"]);
    }

    #[test]
    fn only_a_serial_port_can_be_opened() {
        for good in ["COM3", "COM12", r"\\.\COM15", "/dev/ttyUSB0", "/dev/ttyACM1", "/dev/cu.usbserial-A1", "/dev/pts/4", "/dev/serial/by-id/usb-FTDI_FT232R-if00-port0"] {
            assert!(check_port_name(good).is_ok(), "{good}");
        }
        for bad in ["", "COM", "COMX", "/etc/passwd", "/dev/sda", "/dev/../etc/shadow", "/dev/tty/../../x", "/dev/serial/by-id/../x", r"C:\Windows\win.ini", "NUL", "/dev/pts/x"] {
            assert!(check_port_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_line_is_8n1_at_the_speed_asked_unless_told_otherwise() {
        let line = LineSettings::from_words(9600, None, None, None, None).unwrap();
        assert_eq!(short(&line), "9600 8N1");
        assert_eq!(line.flow, FlowControl::None);
        let other = LineSettings::from_words(115_200, Some(7), Some("even"), Some(2), Some("hardware")).unwrap();
        assert_eq!(short(&other), "115200 7E2");
        assert!(LineSettings::from_words(9601, None, None, None, None).is_err());
        assert!(LineSettings::from_words(9600, Some(9), None, None, None).is_err());
        assert!(LineSettings::from_words(9600, None, Some("mark"), None, None).is_err());
    }

    /// A pseudo-terminal pair stands in for the cable: the far end is the
    /// "device", the near end is opened as the console.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_console_carries_both_ways_and_says_when_the_far_end_goes() {
        use std::io::Write as _;
        let (mut device, near) = serialport::TTYPort::pair().expect("a pty pair");
        let near_name = near.name().expect("the pty has a name");
        drop(near);
        // Opened by name, the way the command opens a real port.
        let port = open_port(&near_name, &LineSettings::from_words(9600, None, None, None, None).unwrap()).expect("the pty opens as a port");
        let mut link = SerialLink::start(port, near_name.clone());

        device.write_all(b"\r\nSwitch>").unwrap();
        let mut got = Vec::new();
        while !got.ends_with(b"Switch>") {
            got.extend(tokio::time::timeout(Duration::from_secs(5), link.read()).await.expect("bytes in time").expect("still open"));
        }

        link.send(b"show version\r").unwrap();
        link.send_break().unwrap();
        let mut typed = Vec::new();
        let mut buf = [0u8; 64];
        device.set_timeout(Duration::from_secs(5)).unwrap();
        while !typed.ends_with(b"\r") {
            let n = device.read(&mut buf).expect("the device hears what was typed");
            typed.extend_from_slice(&buf[..n]);
        }
        assert!(typed.ends_with(b"show version\r"), "{:?}", String::from_utf8_lossy(&typed));

        drop(device);
        let after = tokio::time::timeout(Duration::from_secs(5), async {
            while link.read().await.is_some() {}
        })
        .await;
        assert!(after.is_ok(), "the link notices the far end went");
        assert!(link.why_ended().contains(&near_name), "{}", link.why_ended());
    }
}
