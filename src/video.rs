//! The picture: a V4L2 capture device (an HDMI-to-USB dongle) or the demo screen.
//!
//! One thread owns the source. It publishes the newest JPEG on a watch channel, so a
//! slow viewer skips frames instead of falling behind. MJPEG from the device is passed
//! through (with Huffman tables added); YUYV is encoded here. Nothing is encoded while
//! nobody is watching, and no faster than the configured frame rate.
//!
//! Capture waits for frames with poll() and a timeout, so a dongle that stops sending
//! (HDMI unplugged, the computer asleep) shows "No signal" instead of freezing the
//! thread, and is reopened if it stays silent; one that disappears is looked for again.

use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use linuxvideo::format::{FrameSizes, PixFormat, PixelFormat};
use linuxvideo::{BufType, CapabilityFlags, Device};

use crate::demo;
use crate::mjpeg;
use crate::state::{Frame, Shared, Source, VideoSettings, VideoStatus};

/// No frame for this long: show "No signal".
const NO_SIGNAL_AFTER: Duration = Duration::from_secs(2);
/// No frame for this long: close the device and open it again.
const REOPEN_AFTER: Duration = Duration::from_secs(10);

/// What a source loop returns when it stops.
enum Stop {
    /// The source or the settings changed; pick again now.
    Switch,
    /// The device failed, went away, or went silent.
    Failed(String),
}

/// A capture device worth using: one that streams MJPEG or YUYV.
struct Candidate {
    path: std::path::PathBuf,
    driver: String,
    card: String,
    format: PixelFormat,
    sizes: Vec<(u32, u32)>,
}

pub fn spawn(shared: Arc<Shared>) {
    thread::Builder::new().name("video".into()).spawn(move || run(shared)).expect("video thread");
}

fn run(shared: Arc<Shared>) {
    loop {
        let source = *shared.source.lock().unwrap();
        let why = match (source, find_device()) {
            (Source::Auto, Some(dev)) => match capture(&shared, &dev) {
                Stop::Switch => continue,
                Stop::Failed(e) => {
                    eprintln!("video: {} stopped: {e}", dev.path.display());
                    // Straight back to the device if it is still there: a silent dongle is
                    // reopened, and the demo only shows if it is really gone.
                    if find_device().is_some() {
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    format!("{} went away: {e}", dev.card)
                }
            },
            (Source::Auto, None) => "No capture device found (plug in an HDMI-to-USB dongle).".into(),
            (Source::Demo, _) => "Demo selected.".into(),
        };
        demo_loop(&shared, source, why);
    }
}

fn find_device() -> Option<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    for dev in linuxvideo::list().ok()?.flatten() {
        let Ok(caps) = dev.capabilities() else { continue };
        let flags = caps.device_capabilities();
        if !flags.contains(CapabilityFlags::VIDEO_CAPTURE) || !flags.contains(CapabilityFlags::STREAMING) {
            continue;
        }
        let formats: Vec<PixelFormat> = dev.formats(BufType::VIDEO_CAPTURE).flatten().map(|f| f.pixel_format()).collect();
        let format = if formats.contains(&PixelFormat::MJPG) {
            PixelFormat::MJPG
        } else if formats.contains(&PixelFormat::YUYV) {
            PixelFormat::YUYV
        } else {
            continue;
        };
        let mut sizes: Vec<(u32, u32)> = match dev.frame_sizes(format) {
            Ok(FrameSizes::Discrete(list)) => list.iter().map(|s| (s.width(), s.height())).collect(),
            _ => Vec::new(),
        };
        sizes.retain(|&(w, h)| w >= 320 && h >= 240 && w <= 3840 && h <= 2160);
        sizes.sort_by_key(|&(w, h)| std::cmp::Reverse(w * h));
        sizes.dedup();
        let Ok(path) = dev.path() else { continue };
        found.push(Candidate { path, driver: caps.driver().to_string(), card: caps.card().to_string(), format, sizes });
    }
    // A real USB capture device first, then MJPEG over YUYV, then by name.
    found.sort_by_key(|c| (c.driver != "uvcvideo", c.format != PixelFormat::MJPG, c.path.clone()));
    found.into_iter().next()
}

fn publish_status(shared: &Shared, status: VideoStatus) {
    *shared.video.lock().unwrap() = status;
    shared.changed();
}

fn publish_frame(shared: &Shared, jpeg: Vec<u8>, width: u32, height: u32) {
    let seq = shared.frames.borrow().as_ref().map_or(0, |f| f.seq + 1);
    shared.frames.send_replace(Some(Frame { jpeg: Bytes::from(jpeg), width, height, seq, captured: Instant::now() }));
}

/// Wait until the device has a frame or `timeout` passes. False on timeout.
fn wait_readable(fd: i32, timeout: Duration) -> std::io::Result<bool> {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let n = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::Interrupted { Ok(false) } else { Err(e) };
    }
    if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 && pfd.revents & libc::POLLIN == 0 {
        return Err(std::io::Error::other("device error"));
    }
    Ok(n > 0)
}

/// The size to ask for: the setting if the device offers it, else the largest up to
/// 1080p (720p for YUYV, which is encoded here).
fn pick_size(cand: &Candidate, settings: &VideoSettings) -> (u32, u32) {
    if let Some(s) = settings.size.filter(|s| cand.sizes.contains(s) || cand.sizes.is_empty()) {
        return s;
    }
    let mjpeg = cand.format == PixelFormat::MJPG;
    let cap = if mjpeg { 1920 * 1080 } else { 1280 * 720 };
    let fallback = if mjpeg { (1920, 1080) } else { (1280, 720) };
    cand.sizes.iter().copied().find(|&(w, h)| w * h <= cap).unwrap_or(fallback)
}

fn capture(shared: &Arc<Shared>, cand: &Candidate) -> Stop {
    let fail = |e: std::io::Error| Stop::Failed(e.to_string());
    let settings = shared.settings.lock().unwrap().clone();
    let dev = match Device::open(&cand.path) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let (w, h) = pick_size(cand, &settings);
    let cap = match dev.video_capture(PixFormat::new(w, h, cand.format)) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let fmt = cap.format();
    let (width, height, stride) = (fmt.width(), fmt.height(), fmt.bytes_per_line() as usize);
    let pix = fmt.pixel_format();
    let name = if pix == PixelFormat::MJPG { "MJPEG" } else { "YUYV" };
    let mut stream = match cap.into_stream() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let fd = stream.as_raw_fd();
    eprintln!("video: {} ({}, {}) {width}x{height} {name}", cand.path.display(), cand.driver, cand.card);

    let label = |fps| format!("{width}x{height} {name} {fps}fps");
    let mut status = VideoStatus {
        label: label(0),
        device: format!("{} ({})", cand.card, cand.path.display()),
        width,
        height,
        fps: 0,
        sizes: cand.sizes.clone(),
        signal: true,
    };
    publish_status(shared, status.clone());

    let min_gap = Duration::from_secs_f64(1.0 / f64::from(settings.max_fps.max(1)));
    let (mut frames, mut since) = (0u32, Instant::now());
    let mut last_frame = Instant::now();
    let mut last_sent = Instant::now() - min_gap;
    let mut silent_frames = 0u64;
    loop {
        if *shared.source.lock().unwrap() != Source::Auto || *shared.settings.lock().unwrap() != settings {
            return Stop::Switch;
        }
        match wait_readable(fd, Duration::from_millis(500)) {
            Err(e) => return fail(e),
            Ok(false) => {
                let quiet = last_frame.elapsed();
                if quiet >= REOPEN_AFTER {
                    return Stop::Failed("no frames for 10 s".into());
                }
                if quiet >= NO_SIGNAL_AFTER {
                    if status.signal {
                        status.signal = false;
                        status.fps = 0;
                        status.label = "No signal".into();
                        publish_status(shared, status.clone());
                    }
                    if shared.viewers.load(Ordering::Relaxed) > 0 {
                        silent_frames += 1;
                        let rgb = demo::message("No signal", &format!("{} is not sending a picture", cand.card), silent_frames);
                        let jpeg = mjpeg::encode(&rgb, demo::W as u32, demo::H as u32, jpeg_encoder::ColorType::Rgb, 70);
                        publish_frame(shared, jpeg, demo::W as u32, demo::H as u32);
                    }
                }
                continue;
            }
            Ok(true) => {}
        }
        let wanted = shared.viewers.load(Ordering::Relaxed) > 0 && last_sent.elapsed() >= min_gap;
        let res = stream.dequeue(|buf| {
            if !wanted || buf.is_error() || buf.is_empty() {
                return Ok(None);
            }
            let jpeg = if pix == PixelFormat::MJPG {
                mjpeg::with_huffman_tables(&buf[..])
            } else {
                let need = stride * (height as usize - 1) + width as usize * 2;
                if buf.len() < need {
                    return Ok(None);
                }
                let ycc = mjpeg::yuyv_to_ycbcr(&buf[..], width as usize, height as usize, stride);
                mjpeg::encode(&ycc, width, height, jpeg_encoder::ColorType::Ycbcr, settings.quality)
            };
            Ok(Some(jpeg))
        });
        last_frame = Instant::now();
        match res {
            Ok(Some(jpeg)) if !jpeg.is_empty() => {
                last_sent = Instant::now();
                publish_frame(shared, jpeg, width, height);
            }
            Ok(_) => {}
            Err(e) => return fail(e),
        }
        frames += 1;
        if since.elapsed() >= Duration::from_secs(1) {
            status.fps = frames;
            status.signal = true;
            status.label = label(frames);
            publish_status(shared, status.clone());
            frames = 0;
            since = Instant::now();
        }
    }
}

fn demo_loop(shared: &Arc<Shared>, source: Source, why: String) {
    publish_status(
        shared,
        VideoStatus {
            label: if source == Source::Demo { "Demo screen".into() } else { "No capture, demo".into() },
            device: "demo".into(),
            width: demo::W as u32,
            height: demo::H as u32,
            fps: 10,
            sizes: Vec::new(),
            signal: true,
        },
    );
    shared.demo.lock().unwrap().why = why;
    let mut frame = 0u64;
    let mut last_scan = Instant::now();
    loop {
        let tick = Instant::now();
        if *shared.source.lock().unwrap() != source {
            return;
        }
        // On Auto, look for a device every few seconds so a dongle plugged in later is used.
        if source == Source::Auto && last_scan.elapsed() > Duration::from_secs(3) {
            last_scan = Instant::now();
            if find_device().is_some() {
                return;
            }
        }
        if shared.viewers.load(Ordering::Relaxed) > 0 {
            let quality = shared.settings.lock().unwrap().quality;
            let rgb = shared.demo.lock().unwrap().render(frame);
            let jpeg = mjpeg::encode(&rgb, demo::W as u32, demo::H as u32, jpeg_encoder::ColorType::Rgb, quality);
            publish_frame(shared, jpeg, demo::W as u32, demo::H as u32);
        }
        frame += 1;
        thread::sleep(Duration::from_millis(100).saturating_sub(tick.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(format: PixelFormat, sizes: &[(u32, u32)]) -> Candidate {
        Candidate { path: "/dev/video9".into(), driver: "uvcvideo".into(), card: "t".into(), format, sizes: sizes.to_vec() }
    }

    fn settings(size: Option<(u32, u32)>) -> VideoSettings {
        VideoSettings { max_fps: 30, quality: 80, size }
    }

    #[test]
    fn picks_the_largest_size_up_to_the_cap() {
        let sizes = [(3840, 2160), (1920, 1080), (1280, 720), (640, 480)];
        assert_eq!(pick_size(&cand(PixelFormat::MJPG, &sizes), &settings(None)), (1920, 1080));
        assert_eq!(pick_size(&cand(PixelFormat::YUYV, &sizes), &settings(None)), (1280, 720));
    }

    #[test]
    fn a_chosen_size_is_used_only_if_offered() {
        let sizes = [(1920, 1080), (1280, 720)];
        assert_eq!(pick_size(&cand(PixelFormat::MJPG, &sizes), &settings(Some((1280, 720)))), (1280, 720));
        assert_eq!(pick_size(&cand(PixelFormat::MJPG, &sizes), &settings(Some((800, 600)))), (1920, 1080));
    }
}
