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
    bank: Option<i64>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scan: Duration::from_secs(10),
        write_len: None,
        config_out: None,
        diagnose: false,
        trace: false,
        bank: None,
    };
    let mut argv = std::env::args().skip(1);
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
                let v = argv.next().ok_or("--bank needs a number")?;
                args.bank = Some(v.parse().map_err(|_| "--bank isn't a number")?);
            }
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
  --bank <n>         also run ReadBank for one bank (a smaller read than
                     ReadConfig, so it may fit where the whole config does not)
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
            eprintln!("error: {e}\n\n{HELP}");
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
        return diagnose(&mut guitar).await;
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

    if let Some(n) = args.bank {
        println!(
            "
[extra] ReadBank {n} — a smaller read than the whole config..."
        );
        match guitar
            .call(
                antinode::rpc::Method::ReadBank,
                antinode::rpc::params::bank(n),
            )
            .await
        {
            Ok(v) => {
                println!("        ANSWERED:");
                println!(
                    "{}",
                    serde_json::to_string_pretty(&v).unwrap_or_else(|_| String::from("?"))
                );
            }
            Err(e) => println!("        {e}"),
        }
    }

    if let Some(path) = args.config_out {
        println!("\n[extra] ReadConfig — the live effect catalog...");
        println!("        this is the test of how an over-MTU response arrives (Finding F11).");
        let config = guitar.read_config().await?;
        let pretty = serde_json::to_string_pretty(&config)
            .unwrap_or_else(|_| String::from("<unserializable>"));
        match std::fs::write(&path, &pretty) {
            Ok(()) => println!("        wrote {} bytes to {path}", pretty.len()),
            Err(e) => println!("        could not write {path}: {e}"),
        }
    }

    let _ = guitar.disconnect().await;
    println!("\ndisconnected.");
    Ok(())
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

    let _ = guitar.disconnect().await;
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
             Since it is silent rather than disagreeing, suspect the channel before the\n\
             message format. Run:\n\
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
        _ => "Record what happened in the plan's Findings before changing anything.",
    }
}
