//! Explicit integration with the real Due RPC server and fake native providers.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
#[ignore = "set DUE_RPC_BINARY to an absolute Due binary built with --features rpc; no models used"]
fn async_clients_supervise_both_native_adapters() {
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
                include_bytes!("../../../crates/due-core/tests/fixtures/session-agent.sh"),
            )
            .unwrap();
            fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
            let profile = dir.path().join("profile.json");
            fs::write(
                &profile,
                serde_json::to_vec(&serde_json::json!({
                    "schema_version":1,"provider":provider,"ephemeral":true,
                }))
                .unwrap(),
            )
            .unwrap();
            let script = if language == "js" {
                r#"
                const p=JSON.parse(bridge.args);
                function check(v){if(!v)throw Error('native async assertion');}
                const spec={program:'python3',args:['-c','print("independent check")'],timeout_ms:5000};
                Due.run(async scope=>{
                    let decisions=0,items=0;
                    const worker=await scope.open(p.profile,{
                        onItem(item){items++;},
                        async onInteraction(item,signal){
                            decisions++;
                            check((await scope.exec(spec,signal)).status==='succeeded');
                            return p.provider==='claude'?{behavior:'deny',message:'fixture'}:{decision:'decline'};
                        }
                    });
                    check((await worker.turn('first')).status==='succeeded');
                    check((await scope.exec(spec)).stdout==='independent check\n');
                    check((await worker.turn('ask')).text==='permission denied');
                    check(decisions===1);
                    const turn=await worker.start('hold');
                    await worker.interrupt(turn); await worker.wait(turn);
                    check((await worker.turn('next')).status==='succeeded');
                    check(items>10); bridge.log('NATIVE_ASYNC_OK');
                });
            "#
            } else {
                r#"
                local p=json.decode(bridge.args)
                local function check(v) if not v then error('native async assertion') end end
                local spec={program='python3',args=json.array({'-c','print("independent check")'}),timeout_ms=5000}
                Due.run(function(scope)
                    local decisions,items=0,0
                    local worker=scope:open(p.profile,{
                        onItem=function(item) items=items+1 end,
                        onInteraction=function(item,signal)
                            decisions=decisions+1
                            check(scope:exec(spec,signal).status=='succeeded')
                            if p.provider=='claude' then return {behavior='deny',message='fixture'} end
                            return {decision='decline'}
                        end,
                    })
                    check(worker:turn('first').status=='succeeded')
                    check(scope:exec(spec).stdout=='independent check\n')
                    check(worker:turn('ask').text=='permission denied'); check(decisions==1)
                    local turn=worker:start('hold'); worker:interrupt(turn); worker:wait(turn)
                    check(worker:turn('next').status=='succeeded')
                    check(items>10); bridge.log('NATIVE_ASYNC_OK')
                end)
            "#
            };
            let workflow = dir.path().join("workflow");
            fs::write(&workflow, script).unwrap();
            let input = serde_json::json!({"provider":provider,"profile":{
                "request":profile,"binary":native,"cwd":dir.path(),"timeout":"10s",
            }});
            let output = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
                .arg(language)
                .arg(&workflow)
                .arg("--async")
                .arg("--due")
                .arg(&due)
                .arg("--cwd")
                .arg(dir.path())
                .arg("--evidence")
                .arg(dir.path().join("evidence"))
                .args(["--args", &input.to_string(), "--timeout-ms", "30000"])
                .env("DUE_SESSION_FIXTURE_DIR", dir.path())
                .env("DUE_SESSION_EARLY", "1")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{language}/{provider}: {stderr}");
            assert!(stderr.contains("NATIVE_ASYNC_OK"));
            assert!(dir.path().join("exited").exists());
            let receipt: serde_json::Value = serde_json::from_slice(
                &fs::read(dir.path().join("evidence/receipt.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(receipt["exit_code"], 0);
            assert_eq!(receipt["forced_kill"], false);
            assert!(receipt["transport_error"].is_null());
        }
    }
}

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
                include_bytes!("../../../crates/due-core/tests/fixtures/session-agent.sh"),
            )
            .unwrap();
            fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
            let script = if language == "js" {
                r#"
                const p=JSON.parse(bridge.args); let count=0, approvals=0, provider;
                const c=new DueClient({onItem(i){count++;},onInteraction(i){approvals++;
                    return provider==='claude'?{behavior:'deny',message:'fixture'}:{decision:'cancel'};}});
                function check(v){if(!v)throw Error('native adapter assertion');}
                const opened=c.open(p); provider=opened.provider;
                check(opened.resolved_options.model==='fixture-profile');
                check(c.turn('first').status==='succeeded');
                check(c.turn('ask').text==='permission denied'); check(approvals===1);
                const t=c.start('hold'), id=c.interrupt(t); c.wait(t); c.await(id);
                check(c.turn('next').status==='succeeded');
                c.close(); check(count>10); bridge.log('NATIVE_OK');
            "#
            } else {
                r#"
                local p=json.decode(bridge.args); local count,approvals=0,0; local provider
                local c=DueClient.new({onItem=function(i) count=count+1 end,
                    onInteraction=function(i) approvals=approvals+1
                        if provider=='claude' then return {behavior='deny',message='fixture'} end
                        return {decision='cancel'} end})
                local opened=c:open(p); provider=opened.provider
                assert(opened.resolved_options.model=='fixture-profile')
                assert(c:turn('first').status=='succeeded')
                assert(c:turn('ask').text=='permission denied'); assert(approvals==1)
                local t=c:start('hold'); local id=c:interrupt(t); c:wait(t); c:await(id)
                assert(c:turn('next').status=='succeeded')
                c:close(); assert(count>10); bridge.log('NATIVE_OK')
            "#
            };
            let workflow = dir.path().join("workflow");
            fs::write(&workflow, script).unwrap();
            let request = dir.path().join("profile.json");
            fs::write(
                &request,
                serde_json::to_vec(&serde_json::json!({
                    "schema_version":1,"provider":provider,"model":"fixture-profile",
                    "ephemeral":true,"native_args":[],
                }))
                .unwrap(),
            )
            .unwrap();
            let profile = serde_json::json!({"request":request,"binary":native,"cwd":dir.path(),"timeout":"10s"});
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
