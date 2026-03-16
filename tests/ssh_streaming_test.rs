//! Integration test to reproduce the SSH session hang after execute_streaming.
//!
//! Run with: cargo test --test ssh_streaming_test -- --nocapture
//!
//! This test connects to helios01 (192.168.2.209) and:
//! 1. Runs a regular command (should work)
//! 2. Runs a streaming command that takes ~10s (simulated long command)
//! 3. Immediately runs another regular command (this is where the hang occurs)

use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

// Import from our crate
use whoah::config::HostConfig;
use whoah::ssh::session::SshHost;
use whoah::ssh::RemoteHost;

use whoah::event::BuildEvent;
use whoah::ops::ssh_log::LoggedSsh;

#[tokio::test]
async fn test_command_after_streaming() {
    let config = HostConfig {
        address: "192.168.2.209".to_string(),
        ssh_user: "swherdman".to_string(),
        role: whoah::config::HostRole::Combined,
    };

    println!("Connecting...");
    let host = SshHost::connect(&config).await.expect("SSH connect failed");

    // Step 1: Regular command — should work
    println!("Step 1: Regular command...");
    let out = timeout(Duration::from_secs(10), host.execute("echo hello"))
        .await
        .expect("timeout")
        .expect("execute failed");
    assert_eq!(out.stdout.trim(), "hello");
    println!("  OK: {}", out.stdout.trim());

    // Step 2: Streaming command — simulate a ~10s long-running command
    println!("Step 2: Streaming command (~10s)...");
    let (tx, mut rx) = mpsc::channel::<String>(256);

    let stream_result = timeout(
        Duration::from_secs(30),
        host.execute_streaming("for i in 1 2 3 4 5 6 7 8 9 10; do echo line_$i; sleep 1; done", tx),
    )
    .await
    .expect("streaming timeout")
    .expect("streaming failed");

    // Drain the channel
    rx.close();
    let mut lines = Vec::new();
    while let Some(line) = rx.recv().await {
        lines.push(line);
    }
    println!("  OK: exit={}, lines={}", stream_result, lines.len());

    // Step 3: Regular command immediately after — THIS IS WHERE THE HANG OCCURS
    println!("Step 3: Regular command after streaming...");
    let out = timeout(Duration::from_secs(10), host.execute("echo after_streaming"))
        .await
        .expect("TIMEOUT - session hung after streaming!")
        .expect("execute failed");
    assert_eq!(out.stdout.trim(), "after_streaming");
    println!("  OK: {}", out.stdout.trim());

    // Step 4: Another streaming command to verify session is fully alive
    println!("Step 4: Second streaming command...");
    let (tx2, mut rx2) = mpsc::channel::<String>(256);
    let result2 = timeout(
        Duration::from_secs(15),
        host.execute_streaming("echo second_stream_works", tx2),
    )
    .await
    .expect("second streaming timeout")
    .expect("second streaming failed");
    rx2.close();
    while let Some(_) = rx2.recv().await {}
    println!("  OK: exit={}", result2);

    // Cleanup
    println!("Closing session...");
    host.close().await.expect("close failed");
    println!("All tests passed!");
}

/// Test using LoggedSsh (the actual wrapper used in deploy.rs)
/// This reproduces the exact pattern: run_streaming then run
#[tokio::test]
async fn test_logged_ssh_streaming_then_run() {
    let config = HostConfig {
        address: "192.168.2.209".to_string(),
        ssh_user: "swherdman".to_string(),
        role: whoah::config::HostRole::Combined,
    };

    println!("Connecting...");
    let host = SshHost::connect(&config).await.expect("SSH connect failed");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BuildEvent>();
    // Drain events in background
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match &event {
                BuildEvent::StepDetail(_, msg) => println!("  event: {msg}"),
                _ => {}
            }
        }
    });

    let log_path = std::env::temp_dir().join("whoah-ssh-test.log");
    let mut ssh = LoggedSsh::new(&host, log_path, &tx, "test-step")
        .await
        .expect("LoggedSsh::new failed");

    // Step 1: Regular run
    println!("Step 1: LoggedSsh::run...");
    let out = timeout(Duration::from_secs(10), ssh.run("echo logged_hello"))
        .await
        .expect("timeout")
        .expect("run failed");
    println!("  OK: {}", out.stdout.trim());

    // Step 2: Streaming for 15 seconds
    println!("Step 2: LoggedSsh::run_streaming (15s)...");
    let exit = timeout(
        Duration::from_secs(30),
        ssh.run_streaming("for i in $(seq 1 15); do echo logged_line_$i; sleep 1; done"),
    )
    .await
    .expect("streaming timeout")
    .expect("streaming failed");
    println!("  OK: exit={exit}");

    // Step 3: Regular run immediately after — the critical test
    println!("Step 3: LoggedSsh::run after streaming...");
    let out = timeout(Duration::from_secs(10), ssh.run("echo after_logged_streaming"))
        .await
        .expect("TIMEOUT - LoggedSsh hung after streaming!")
        .expect("run failed");
    println!("  OK: {}", out.stdout.trim());
    assert_eq!(out.stdout.trim(), "after_logged_streaming");

    host.close().await.expect("close failed");
    println!("LoggedSsh test passed!");
}

#[tokio::test]
async fn test_long_streaming_then_command() {
    let config = HostConfig {
        address: "192.168.2.209".to_string(),
        ssh_user: "swherdman".to_string(),
        role: whoah::config::HostRole::Combined,
    };

    println!("Connecting...");
    let host = SshHost::connect(&config).await.expect("SSH connect failed");

    // Simulate a command closer to pkg install duration (~60s)
    println!("Running 60s streaming command...");
    let (tx, mut rx) = mpsc::channel::<String>(256);

    let stream_result = timeout(
        Duration::from_secs(90),
        host.execute_streaming(
            "for i in $(seq 1 60); do echo progress_$i; sleep 1; done",
            tx,
        ),
    )
    .await
    .expect("streaming timeout")
    .expect("streaming failed");

    rx.close();
    let mut count = 0;
    while let Some(_) = rx.recv().await {
        count += 1;
    }
    println!("  Streaming done: exit={}, lines={}", stream_result, count);

    // Now try a regular command
    println!("Running command after 60s stream...");
    let out = timeout(Duration::from_secs(10), host.execute("echo still_alive"))
        .await
        .expect("TIMEOUT - session hung after 60s streaming!")
        .expect("execute failed");
    println!("  Result: {}", out.stdout.trim());
    assert_eq!(out.stdout.trim(), "still_alive");

    host.close().await.expect("close failed");
    println!("All tests passed!");
}
