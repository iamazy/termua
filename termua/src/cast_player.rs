use std::{
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

use anyhow::Context;

use crate::env::{CAST_PLAYER_ENV_MODE, CAST_PLAYER_ENV_PATH, CAST_PLAYER_ENV_SPEED};

#[derive(Debug, Clone, PartialEq)]
enum CastEvent {
    Output { t: f64, text: String },
    Input { t: f64, text: String },
    Resize { t: f64, size: String },
}

#[derive(Debug, Clone, Copy)]
pub struct PlayOptions {
    pub speed: f64,
    pub sleep: bool,
}

impl Default for PlayOptions {
    fn default() -> Self {
        Self {
            speed: 1.0,
            sleep: true,
        }
    }
}

fn parse_event_line(line: &str) -> anyhow::Result<CastEvent> {
    let v: serde_json::Value =
        serde_json::from_str(line).with_context(|| format!("invalid cast event json: {line:?}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("cast event must be a JSON array"))?;
    if arr.len() != 3 {
        return Err(anyhow::anyhow!(
            "cast event must have 3 items, got {}",
            arr.len()
        ));
    }

    let t = arr[0]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("cast event t must be a number"))?;
    let ty = arr[1]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("cast event type must be a string"))?;
    let data = arr[2]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("cast event data must be a string"))?;

    match ty {
        "o" => Ok(CastEvent::Output {
            t,
            text: data.to_string(),
        }),
        "i" => Ok(CastEvent::Input {
            t,
            text: data.to_string(),
        }),
        "r" => Ok(CastEvent::Resize {
            t,
            size: data.to_string(),
        }),
        other => Err(anyhow::anyhow!("unsupported cast event type: {other:?}")),
    }
}

pub fn play_cast<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    opts: PlayOptions,
) -> anyhow::Result<()> {
    if !(opts.speed.is_finite() && opts.speed > 0.0) {
        return Err(anyhow::anyhow!("speed must be > 0"));
    }

    let mut header = String::new();
    let n = reader.read_line(&mut header)?;
    if n == 0 {
        return Err(anyhow::anyhow!("empty cast file"));
    }
    let header_trimmed = header.trim();
    if !header_trimmed.is_empty() {
        let v: serde_json::Value = serde_json::from_str(header_trimmed)
            .context("invalid cast header json (expected first line to be header)")?;
        if v.get("version")
            .and_then(|v| v.as_u64())
            .is_some_and(|ver| ver != 2)
        {
            return Err(anyhow::anyhow!("unsupported cast version (expected 2)"));
        }
    }

    let start = Instant::now();
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        let line = buf.trim_end();
        if line.is_empty() {
            continue;
        }

        let ev = parse_event_line(line)?;

        if opts.sleep {
            let t = match &ev {
                CastEvent::Output { t, .. } => *t,
                CastEvent::Input { t, .. } => *t,
                CastEvent::Resize { t, .. } => *t,
            };
            if t.is_finite() && t > 0.0 {
                let target = start + Duration::from_secs_f64(t / opts.speed);
                let now = Instant::now();
                if target > now {
                    std::thread::sleep(target - now);
                }
            }
        }

        match ev {
            CastEvent::Output { text, .. } => {
                writer.write_all(text.as_bytes())?;
                writer.flush()?;
            }
            CastEvent::Input { .. } => {}
            CastEvent::Resize { .. } => {}
        }
    }

    // Playback finished marker.
    writer.write_all(b"\r\n[Recorder] Playback finished.\r\n")?;
    writer.flush()?;

    // Playback finished: stop cursor blinking and hide it, so the recorder tab looks "frozen".
    // - ?12l: disable blinking cursor (DECTCEM blinking, xterm extension).
    // - ?25l: hide cursor (DECTCEM).
    writer.write_all(b"\x1b[?12l\x1b[?25l")?;
    writer.flush()?;

    Ok(())
}

pub fn try_run_from_env() -> anyhow::Result<bool> {
    if std::env::var(CAST_PLAYER_ENV_MODE).ok().as_deref() == Some("1")
        && let Ok(path) = std::env::var(CAST_PLAYER_ENV_PATH)
        && !path.trim().is_empty()
    {
        let mut opts = PlayOptions::default();
        if let Ok(speed) = std::env::var(CAST_PLAYER_ENV_SPEED)
            && !speed.trim().is_empty()
        {
            opts.speed = speed.parse::<f64>()?;
        }

        let f = std::fs::File::open(path)?;
        let r = io::BufReader::new(f);
        let mut out = io::stdout().lock();
        play_cast(r, &mut out, opts)?;
        return Ok(true);
    }

    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        return Ok(false);
    };
    if first != "--play-cast" {
        return Ok(false);
    }

    let Some(path) = args.next() else {
        return Err(anyhow::anyhow!(
            "usage: termua --play-cast <path.cast> [--speed <n>]"
        ));
    };

    let mut opts = PlayOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--speed" => {
                let v = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--speed requires a value"))?;
                opts.speed = v.parse::<f64>()?;
            }
            _ => return Err(anyhow::anyhow!("unknown arg: {arg}")),
        }
    }

    let f = std::fs::File::open(path)?;
    let r = io::BufReader::new(f);
    let mut out = io::stdout().lock();
    play_cast(r, &mut out, opts)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_cast_writes_output_in_order_without_sleep() {
        let input = r#"{"version":2,"width":80,"height":24,"timestamp":0}
[0.0,"o","hi"]
[0.1,"o"," there"]
"#;
        let mut out = Vec::<u8>::new();
        play_cast(
            io::BufReader::new(input.as_bytes()),
            &mut out,
            PlayOptions {
                speed: 1.0,
                sleep: false,
            },
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "hi there\r\n[Recorder] Playback finished.\r\n{}",
                "\u{1b}[?12l\u{1b}[?25l"
            )
        );
    }
}
