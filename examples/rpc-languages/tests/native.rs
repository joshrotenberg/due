//! Explicit integration with the real Due RPC server and fake native providers.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
#[ignore = "set DUE_RPC_BINARY to an absolute Due binary built with --features rpc; no models used"]
fn both_clients_use_both_native_adapters() {
    let due = std::env::var_os("DUE_RPC_BINARY").expect("DUE_RPC_BINARY is required");
    for language in [
        #[cfg(feature = "js")]
        "js",
        #[cfg(feature = "lua")]
        "lua",
    ] {
        for provider in ["claude", "codex"] {
            let dir = tempfile::tempdir().unwrap();
            let native = dir.path().join("native");
            fs::write(
                &native,
                include_bytes!("../../../tests/fixtures/session-agent.sh"),
            )
            .unwrap();
            fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
            let script = if language == "js" {
                r#"
                const p=JSON.parse(bridge.args); let count=0, approvals=0;
                const c=new DueClient({onItem(i){count++;},onInteraction(i){approvals++;
                    return p.provider==='claude'?{behavior:'deny',message:'fixture'}:{decision:'cancel'};}});
                function check(v){if(!v)throw Error('native adapter assertion');}
                c.open(p);
                check(c.turn('first').status==='succeeded');
                check(c.turn('ask').text==='permission denied'); check(approvals===1);
                const t=c.start('hold'), id=c.interrupt(t); c.wait(t); c.await(id);
                check(c.turn('next').status==='succeeded');
                c.close(); check(count>10); bridge.log('NATIVE_OK');
            "#
            } else {
                r#"
                local p=json.decode(bridge.args); local count,approvals=0,0
                local c=DueClient.new({onItem=function(i) count=count+1 end,
                    onInteraction=function(i) approvals=approvals+1
                        if p.provider=='claude' then return {behavior='deny',message='fixture'} end
                        return {decision='cancel'} end})
                c:open(p)
                assert(c:turn('first').status=='succeeded')
                assert(c:turn('ask').text=='permission denied'); assert(approvals==1)
                local t=c:start('hold'); local id=c:interrupt(t); c:wait(t); c:await(id)
                assert(c:turn('next').status=='succeeded')
                c:close(); assert(count>10); bridge.log('NATIVE_OK')
            "#
            };
            let workflow = dir.path().join("workflow");
            fs::write(&workflow, script).unwrap();
            let profile = serde_json::json!({"provider":provider,"binary":native,"cwd":dir.path(),"timeout":"10s"});
            let output = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
                .arg(language)
                .arg(&workflow)
                .arg("--due")
                .arg(&due)
                .arg("--cwd")
                .arg(dir.path())
                .arg("--evidence")
                .arg(dir.path().join("evidence"))
                .args(["--args", &profile.to_string(), "--timeout-ms", "30000"])
                .env("DUE_SESSION_FIXTURE_DIR", dir.path())
                .env("DUE_SESSION_EARLY", "1")
                .output()
                .unwrap();
            if !output.status.success() {
                let kept = dir.keep();
                panic!(
                    "{language}/{provider}: {}\nevidence: {}",
                    String::from_utf8_lossy(&output.stderr),
                    kept.display()
                );
            }
            assert!(String::from_utf8_lossy(&output.stderr).contains("NATIVE_OK"));
            assert!(
                dir.path().join("exited").exists(),
                "native child did not exit"
            );
            let receipt: serde_json::Value = serde_json::from_slice(
                &fs::read(dir.path().join("evidence/receipt.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(receipt["exit_code"], 0);
            assert_eq!(receipt["forced_kill"], false);
            assert_eq!(receipt["transport_error"], serde_json::Value::Null);
        }
    }
}
