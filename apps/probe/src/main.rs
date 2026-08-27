//! The Phase 1 control run.
//!
//! Everything antinode knows about the protocol was read out of a decompiled
//! application, which makes it a hypothesis. This is the experiment that tests
//! it: connect to a real guitar, read its version banner, ask for its status,
//! and print what came back next to what was predicted.
//!
//! A `GetStatus` that answers promotes the whole recovered map from
//! static-read to hardware-verified in one shot. A `GetStatus` that does not
//! answer is just as informative, and the output is written so the *first*
//! step that diverged is obvious rather than buried.
//!
//! Usage:
//!
//! ```text
//! antinode-probe                       scan, connect, banner + status
//! antinode-probe --config out.json     also dump the live effect catalog
//! antinode-probe --write-len 244       override the assumed write length
//! antinode-probe --scan-secs 20        scan for longer
//! ```

use std::time::Duration;

use antinode_ble::{Guitar, MatchedBy, TransportError, discover};

struct Args {
    scan: Duration,
    write_len: Option<usize>,
    config_out: Option<String>,
    diagnose: bool,
    trace: bool,
    bank: Option<(i64, i64)>,
    call: Vec<(String, String)>,
    sweep: bool,
}

/// Parse call parameters as JSON, or as comma-separated `key=value` pairs.
///
/// The `key=value` form exists because PowerShell strips the quotes out of
/// `{"bank_num":0}` before the program ever sees it, leaving `{bank_num:0}`,
/// which is not JSON. Rather than teach every user a shell-quoting rule,
/// accept a form that needs no quotes at all.
fn parse_params(text: &str) -> Result<serde_json::Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(serde_json::json!({}));
    }
    if text.starts_with('{') {
        // Tolerate the unquoted-key shape PowerShell produces.
        return serde_json::from_str(text).or_else(|_| {
            let repaired = repair_unquoted_keys(text);
            serde_json::from_str(&repaired)
                .map_err(|e| format!("not JSON ({e}); try key=value form instead, e.g. bank_num=0"))
        });
    }
    let mut map = serde_json::Map::new();
    for pair in text.split(',') {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| format!("`{pair}` is not key=value"))?;
        let value = if let Ok(n) = v.trim().parse::<i64>() {
            serde_json::json!(n)
        } else if let Ok(f) = v.trim().parse::<f64>() {
            serde_json::json!(f)
        } else if v.trim() == "true" || v.trim() == "false" {
            serde_json::json!(v.trim() == "true")
        } else {
            serde_json::json!(v.trim())
        };
        map.insert(k.trim().to_string(), value);
    }
    Ok(serde_json::Value::Object(map))
}

/// Put quotes back around bare object keys, undoing what a shell removed.
fn repair_unquoted_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if c == '"' {
            in_string = !in_string;
            out.push(c);
            continue;
        }
        if !in_string && (c.is_ascii_alphabetic() || c == '_') {
            let mut word = String::new();
            word.push(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '_' {
                    word.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let is_key = matches!(chars.peek(), Some(':'));
            let literal = matches!(word.as_str(), "true" | "false" | "null");
            if is_key && !literal {
                out.push('"');
                out.push_str(&word);
                out.push('"');
            } else {
                out.push_str(&word);
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Accept either `3` or `0-7`.
fn parse_range(text: &str) -> Result<(i64, i64), String> {
    if let Some((a, b)) = text.split_once('-') {
        let first: i64 = a.trim().parse().map_err(|_| "range start isn't a number")?;
        let last: i64 = b.trim().parse().map_err(|_| "range end isn't a number")?;
        if last < first {
            return Err(String::from("range end is before its start"));
        }
        Ok((first, last))
    } else {
        let n: i64 = text.trim().parse().map_err(|_| "--bank isn't a number")?;
        Ok((n, n))
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scan: Duration::from_secs(10),
        write_len: None,
        config_out: None,
        diagnose: false,
        trace: false,
        bank: None,
        call: Vec::new(),
        sweep: false,
    };
    let mut argv = std::env::args().skip(1).peekable();
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--scan-secs" => {
                let v = argv.next().ok_or("--scan-secs needs a value")?;
                args.scan =
                    Duration::from_secs(v.parse().map_err(|_| "--scan-secs isn't a number")?);
            }
            "--write-len" => {
                let v = argv.next().ok_or("--write-len needs a value")?;
                args.write_len = Some(v.parse().map_err(|_| "--write-len isn't a number")?);
            }
            "--bank" => {
                let v = argv
                    .next()
                    .ok_or("--bank needs a number or range like 0-7")?;
                args.bank = Some(parse_range(&v)?);
            }
            "--call" => {
                let m = argv.next().ok_or("--call needs a method name")?;
                // Only take a following argument as params if it could be
                // params. Swallowing the next flag turns a valid command line
                // into a baffling error about an argument the user did write.
                let p = match argv.peek() {
                    Some(next) if !next.starts_with("--") => argv.next().unwrap(),
                    _ => String::from("{}"),
                };
                args.call.push((m, p));
            }
            "--sweep" => args.sweep = true,
            "--diagnose" => args.diagnose = true,
            "--trace" => args.trace = true,
            "--config" => {
                args.config_out = Some(argv.next().ok_or("--config needs a path")?);
            }
            "-h" | "--help" => {
                println!("{}", HELP);
                std::process::exit(0);
            }
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }
    Ok(args)
}

const HELP: &str = "\
antinode-probe — confirm the recovered HyVibe protocol against a real guitar

  --scan-secs <n>    how long to scan (default 10)
  --write-len <n>    override the assumed write length (default 514)
  --config <path>    also run ReadConfig and write the result here
  --bank <n|a-b>     also run ReadBank for one bank or a range (e.g. 0-7)
  --call <m> [json]  call any method by wire name, including ones antinode has
                     no variant for (e.g. --call PrintBank '{\"bank_num\":0}').
                     Repeatable. Anything arriving after the reply is reported.
  --sweep            ask the device which of the dictionary's undocumented
                     methods actually exist. Query-shaped methods only — the
                     ones that would change the instrument are never swept.
  --trace            print every notification as it arrives
  --diagnose         if GetStatus is unanswered, try candidate encodings and
                     report which, if any, the device replies to
  -h, --help         this text

Turn the guitar on and make sure the phone app is NOT connected to it: the
instrument accepts one connection at a time, and the app will hold it.";

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            // Deliberately not the full help. It ends with advice about the
            // guitar's connection state, which reads as a diagnosis of the
            // instrument when the only thing wrong is the command line.
            eprintln!("error: {e}");
            eprintln!("Nothing was sent to the guitar. Run --help for usage.");
            std::process::exit(2);
        }
    };

    if let Err(e) = run(args).await {
        eprintln!("\nFAILED: {e}");
        eprintln!("\n{}", advice(&e));
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), TransportError> {
    println!("antinode probe — Phase 1 control run");
    println!("everything below is predicted from static analysis until it answers\n");

    println!("[1/4] scanning {:?}...", args.scan);
    println!("      looking for service {}", antinode::GUITAR_SERVICE);
    println!("      or a device named like the System Menu's BT ID (e.g. H2-SE614)");
    let found = discover(args.scan).await?;
    for (i, f) in found.iter().enumerate() {
        let how = match f.matched_by {
            MatchedBy::AdvertisedService => "advertises the guitar service",
            MatchedBy::Name => "name only — service not in the advertisement",
        };
        println!(
            "      found #{i}: {} [{}] — {how}",
            f.name.as_deref().unwrap_or("(unnamed)"),
            f.address
        );
    }
    if found[0].matched_by == MatchedBy::AdvertisedService {
        println!("      ADVERTISEMENT CONFIRMED — the service UUID is in the advertisement");
    } else {
        println!("      matched by name; whether the service exists is settled on connect");
    }

    println!("\n[2/4] connecting to #0...");
    let mut guitar = Guitar::connect(&found[0]).await?;
    if let Some(len) = args.write_len {
        guitar.set_write_len(len);
    }
    guitar.set_trace(args.trace || args.diagnose);
    println!("      connected; GATT surface has both expected characteristics");
    println!(
        "      write length in use: {} (assumed, not negotiated)",
        guitar.write_len()
    );

    // Everything past this point runs inside `session`, so a failure still
    // releases the connection. The instrument accepts one client at a time, so
    // a leaked connection does not merely waste a handle: it locks out the next
    // run, and the symptom is some *other* request appearing to fail. An early
    // `?` here would make this probe the cause of the next probe's failure.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let outcome = session(&mut guitar, &args).await;

    let _ = guitar.disconnect().await;
    println!("\ndisconnected.");
    outcome
}

/// The work done while connected. The caller always disconnects afterwards,
/// whatever this returns.
async fn session(guitar: &mut Guitar, args: &Args) -> Result<(), TransportError> {
    println!("\n[3/4] reading the version banner (a GATT read, before any RPC)...");
    match guitar.banner().await {
        Ok(banner) => {
            println!("      BANNER CONFIRMED");
            println!(
                "      audio DSP (STM): {}{}",
                banner.stm,
                if banner.stm_was_implied {
                    "  <- assumed, not reported"
                } else {
                    ""
                }
            );
            println!("      connectivity (ESP): {}", banner.esp);
        }
        Err(e) => {
            println!("      banner did not parse: {e}");
            println!("      (continuing — the banner is not required for RPC)");
        }
    }

    if args.diagnose {
        return diagnose(guitar).await;
    }

    println!("\n[4/4] GetStatus — the control run...");
    let status = guitar.status().await?;
    println!("      *** THE PROTOCOL MAP IS CONFIRMED AGAINST HARDWARE ***\n");
    println!("      device          {}", status.device);
    println!("      cpu id          {}", status.cpu_id);
    println!("      battery         {:.0}%", status.battery_percent);
    println!(
        "      free space      {:.2} GB ({:.1}% free)",
        status.free_space_gb,
        status.free_space_fraction * 100.0
    );
    println!("      firmware ESP    {}", status.version_esp);
    println!("      firmware STM    {}", status.version_stm);
    println!("\n      cpu id and STM version are what a firmware assessment would start from.");

    if let Some((first, last)) = args.bank {
        println!(
            "
[extra] ReadBank {first}..={last}"
        );
        println!("        the device returns a *string* per bank, and the vendor's own app");
        println!("        never calls this, so its semantics are ours to establish.");
        let mut answered = 0;
        let mut nonempty = 0;
        for n in first..=last {
            match guitar
                .call(
                    antinode::rpc::Method::ReadBank,
                    antinode::rpc::params::bank(n),
                )
                .await
            {
                Ok(v) => {
                    answered += 1;
                    let rendered = match v.as_str() {
                        Some("") => String::from("(empty string)"),
                        Some(text) => {
                            nonempty += 1;
                            format!("{} chars: {text}", text.len())
                        }
                        None => {
                            nonempty += 1;
                            format!(
                                "not a string: {}",
                                serde_json::to_string(&v).unwrap_or_default()
                            )
                        }
                    };
                    println!("        bank {n}: {rendered}");
                    // A reply may be an acknowledgement with the payload
                    // following separately; check rather than assume.
                    let trailing = guitar.drain(Duration::from_millis(600)).await;
                    for line in &trailing {
                        println!("          + trailing: {line}");
                    }
                }
                Err(e) => println!("        bank {n}: {e}"),
            }
        }
        println!(
            "
        {answered} of {} answered, {nonempty} with content.",
            last - first + 1
        );
        if answered > 0 && nonempty == 0 {
            println!(
                "        Every bank answered but all were empty, so ReadBank is reachable
                         and either the banks are genuinely empty or their content lives
                         somewhere other than this result. Reachability is what this tested."
            );
        }
    }

    if args.sweep {
        sweep_methods(guitar).await;
    }

    for (method, params_json) in &args.call {
        println!(
            "
[extra] {method} — an arbitrary method call"
        );
        let params = match parse_params(params_json) {
            Ok(v) => v,
            Err(e) => {
                println!("        bad params: {e}");
                continue;
            }
        };
        println!("        params: {params}");
        match guitar.call_named(method, params).await {
            Ok(v) => {
                println!(
                    "        ANSWERED: {}",
                    serde_json::to_string(&v).unwrap_or_default()
                );
                if let Some(dump) = render_maybe_hex(&v) {
                    print!("{dump}");
                }
            }
            Err(e) => println!("        {e}"),
        }
        let trailing = guitar.drain(Duration::from_millis(800)).await;
        if trailing.is_empty() {
            println!("        (nothing followed)");
        } else {
            for line in &trailing {
                println!("        + trailing: {line}");
            }
        }
    }

    if let Some(path) = args.config_out.as_deref() {
        println!("\n[extra] ReadConfig — the live effect catalog...");
        println!("        this is the test of how an over-MTU response arrives (Finding F11).");
        let config = guitar.read_config().await?;
        let pretty = serde_json::to_string_pretty(&config)
            .unwrap_or_else(|_| String::from("<unserializable>"));
        match std::fs::write(path, &pretty) {
            Ok(()) => println!("        wrote {} bytes to {path}", pretty.len()),
            Err(e) => println!("        could not write {path}: {e}"),
        }
    }

    Ok(())
}

/// Render a hex-string reply as bytes, since some methods answer that way.
///
/// `DumpFile` returns file contents as an uppercase hex string. Printed raw it
/// is an unreadable wall; decoded, a WAV header or a JSON fragment identifies
/// itself at a glance. Anything that is not plausibly hex is left alone.
fn render_maybe_hex(value: &serde_json::Value) -> Option<String> {
    let text = value.as_str()?;
    if text.len() < 8 || text.len() % 2 != 0 || !text.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect();
    if bytes.len() * 2 != text.len() {
        return None;
    }

    let mut out = format!(
        "        decoded {} bytes:
",
        bytes.len()
    );
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        out.push_str(&format!(
            "          {:04x}  {:<47}  |{ascii}|
",
            row * 16,
            hex.join(" ")
        ));
    }
    Some(out)
}

/// Methods the compressor's dictionary names but the vendor's app never calls,
/// restricted to those whose names promise only to *report* something.
///
/// The mutating remainder — `ActivateSpkFilter`, `BypassEffect`, `DumpFile`,
/// `LaunchCalibration`, `PullFbk`, `SetGainPreamp`, `SetPhaseInv`,
/// `SetSpeakerBiquads`, `StartAnalysis`, `StartRendering`, `StartTuner`,
/// `StopRendering`, `StopTuner` — is deliberately absent. Sweeping those would
/// mean firing thirteen state-changing commands with guessed parameters at
/// someone's instrument to satisfy curiosity. They are one `--call` away when
/// there is a reason to want one.
const QUERY_METHODS: &[&str] = &[
    "BTcheck",
    "GetAnalysis",
    "GetLastRecordingName",
    "GetLevels",
    "PrintBank",
    "ReadMetronome",
];

/// Ask the device which methods it implements.
///
/// The keyword dictionary proves the firmware knows a string; it does not prove
/// there is a method behind it. The device settles that itself — an unknown
/// method comes back as error 4, "Method not found", which makes this a real
/// membership test rather than a guess.
async fn sweep_methods(guitar: &mut Guitar) {
    println!(
        "
[extra] method sweep — which undocumented methods exist?"
    );
    println!("        query-shaped methods only; nothing here changes the instrument.");

    let mut exists = Vec::new();
    let mut missing = Vec::new();

    for name in QUERY_METHODS {
        match guitar.call_named(name, serde_json::json!({})).await {
            Ok(v) => {
                println!(
                    "        {name}: EXISTS — {}",
                    serde_json::to_string(&v).unwrap_or_default()
                );
                exists.push(*name);
            }
            Err(TransportError::Rpc(antinode::rpc::RpcError::Device(e))) => {
                // Code 4 is the device's "Method not found"; any other code
                // means the method is real and merely objected to the call.
                if e.code == 4.0 {
                    println!("        {name}: not implemented");
                    missing.push(*name);
                } else {
                    println!(
                        "        {name}: EXISTS — but rejected: {} ({})",
                        e.message, e.code
                    );
                    exists.push(*name);
                }
            }
            Err(e) => println!("        {name}: {e}"),
        }
        let trailing = guitar.drain(Duration::from_millis(400)).await;
        for line in &trailing {
            println!("          + trailing: {line}");
        }
    }

    println!(
        "
        {} implemented, {} absent.",
        exists.len(),
        missing.len()
    );
    if !exists.is_empty() {
        println!("        implemented: {}", exists.join(", "));
        println!("        A method that exists but rejected an empty call wants parameters;");
        println!("        its error message is usually the best clue about which.");
    }
}

/// Try each candidate request encoding and report which the device answers.
///
/// The recovered encoding is a hypothesis, and when it goes unanswered the
/// useful next step is not to guess again but to test the small number of
/// things it could plausibly be, one at a time, and say what happened to each.
async fn diagnose(guitar: &mut Guitar) -> Result<(), TransportError> {
    const LISTEN: Duration = Duration::from_secs(4);

    println!(
        "
[4/4] diagnostic mode — the recovered encoding went unanswered
"
    );

    println!("  first: does the device ever speak unprompted?");
    let idle = guitar.listen(LISTEN).await;
    if idle.is_empty() {
        println!(
            "    nothing in {LISTEN:?} — expected; it should only answer when asked.
"
        );
    } else {
        println!(
            "    it sent {} message(s) with no request: {idle:?}
",
            idle.len()
        );
    }

    // Each candidate differs from the recovered encoding in exactly one way, so
    // whichever answers names the single wrong assumption.
    let candidates: [(&str, String, bool); 5] = [
        (
            "recovered: numeric jsonrpc, write-with-response",
            r#"{"jsonrpc":2.0,"id":90,"method":"GetStatus","params":{}}"#.to_string(),
            true,
        ),
        (
            "same, but write-WITHOUT-response",
            r#"{"jsonrpc":2.0,"id":91,"method":"GetStatus","params":{}}"#.to_string(),
            false,
        ),
        (
            "newline-terminated (as LLT frames are)",
            "{\"jsonrpc\":2.0,\"id\":92,\"method\":\"GetStatus\",\"params\":{}}
"
            .to_string(),
            true,
        ),
        (
            "spec-compliant STRING jsonrpc (contradicts F12)",
            r#"{"jsonrpc":"2.0","id":93,"method":"GetStatus","params":{}}"#.to_string(),
            true,
        ),
        (
            "no params field at all",
            r#"{"jsonrpc":2.0,"id":94,"method":"GetStatus"}"#.to_string(),
            true,
        ),
    ];

    let mut answered = Vec::new();
    for (label, payload, with_response) in &candidates {
        println!("  trying: {label}");
        println!("    -> {} bytes: {payload:?}", payload.len());
        match guitar.write_raw(payload.as_bytes(), *with_response).await {
            Ok(()) => {
                let heard = guitar.listen(LISTEN).await;
                if heard.is_empty() {
                    println!(
                        "    silence.
"
                    );
                } else {
                    println!(
                        "    ANSWERED with {} message(s)
",
                        heard.len()
                    );
                    answered.push((*label, heard));
                }
            }
            Err(e) => println!(
                "    the write itself failed: {e}
"
            ),
        }
    }

    println!(
        "
=== diagnosis ==="
    );
    if answered.is_empty() {
        println!(
            "No encoding drew a reply, and the device never spoke unprompted.
             That points away from the message format and toward the channel itself:
             - notifications may not really be enabled (the CCCD write may not have taken),
             - or writes are not reaching the device despite appearing to succeed, which
               is what an unnegotiated MTU would do to a 55-byte write.
             Next: confirm with a generic BLE tool (nRF Connect) that writing this exact
             payload to the request characteristic produces a notification at all."
        );
    } else {
        println!("These encodings drew a reply:");
        for (label, heard) in &answered {
            println!("  - {label}");
            for line in heard {
                println!("      {line:?}");
            }
        }
        println!(
            "
The one that answered names the assumption to correct in Findings."
        );
    }

    Ok(())
}

/// Turn a failure into the next thing worth trying.
///
/// A probe that fails is still an experiment with a result; what makes it
/// useful is saying which assumption it falsified.
fn advice(e: &TransportError) -> &'static str {
    match e {
        TransportError::NotFound(_) => {
            "Nothing in range looked like a guitar.\n\
             The scan is unfiltered and matches on the service UUID *or* the name, so\n\
             this is not a scan-filter false negative. Check, in order:\n\
             - Is it powered on, and is USB mode OFF? USB mode makes it a mass-storage\n\
               drive, not a Bluetooth peripheral.\n\
             - Is the phone app connected? It holds the one connection the guitar has.\n\
             - Is it paired as a Bluetooth *speaker* in the OS? That is Bluetooth Classic\n\
               audio, a different radio path from the app's BLE control link. Unpair it.\n\
             - Check the System Menu for the BT ID (e.g. H2-SE614). If the name does not\n\
               start with H2- and does not contain 'hyvibe', tell antinode what to match."
        }
        TransportError::NoAdapter => "No Bluetooth adapter. Is Bluetooth switched on?",
        TransportError::MissingCharacteristic(_) => {
            "Connected, but the characteristic UUIDs are not what was recovered.\n\
             This falsifies Finding F1 — the map is wrong or this is different firmware.\n\
             Dump the real GATT table and record it before going further."
        }
        TransportError::Timeout { heard, .. } if heard.is_empty() => {
            "The write succeeded and the device said nothing at all.\n\
             \n\
             If this request worked on an earlier run, suspect device state before the\n\
             code: a previous run that ended in an error may have left the instrument\n\
             mid-transfer, and it serves one client at a time. Power-cycle the guitar\n\
             (hold the knob two seconds, then tap it) and try again — a stuck transfer\n\
             shows up as some *other* request failing, which is a misleading symptom.\n\
             \n\
             If it has never worked, the request may need LLT2: this firmware selects a\n\
             compressed, binary-framed transport for anything large, and antinode only\n\
             speaks the older one. Small requests work either way, which is why\n\
             GetStatus is not evidence that a bigger read will.\n\
             \n\
             Otherwise, run:\n\
             \n\
                 cargo run -p antinode-probe -- --diagnose\n\
             \n\
             which tries each candidate encoding in turn and reports which, if any, it\n\
             answers — including whether it ever speaks unprompted at all."
        }
        TransportError::Timeout { .. } => {
            "The device replied, but with nothing this client recognised as the answer.\n\
             That is much closer in than silence: the channel works and the device is\n\
             talking. The messages it sent are printed in the error above — compare them\n\
             against the expected envelope and correct the Findings accordingly."
        }
        TransportError::ChunkRejected { .. } => {
            "The device rejected a frame of a split message, which means it IS speaking\n\
             LLT — good — but our framing is off. The status code names which invariant\n\
             it disliked (sequence, object id, or the frame itself)."
        }
        TransportError::Rpc(_) => {
            "The device answered and the envelope parsed, so the transport is right.\n\
             This is a protocol-level disagreement about one method — much closer in\n\
             than a silent failure. Record the exact error."
        }
        TransportError::Bluetooth(_) => {
            "The Bluetooth stack refused the operation. If this was 'Not connected' during
             connect, the peripheral handle from the scan had gone stale, or the guitar
             dropped between being seen and being connected to — antinode now retries
             that a few times, so a failure here means it failed repeatedly.
             
             Check the guitar is awake, then try again. If Windows has gotten confused,
             toggling Bluetooth off and on clears its device cache."
        }
        _ => "Record what happened in the plan's Findings before changing anything.",
    }
}
