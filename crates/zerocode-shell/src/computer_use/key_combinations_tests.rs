fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn press_key_combinations_run_as_hotkey_once() {
    use super::call_with_the_persons_last_step;
    use crate::computer_use::confirm::Asking;
    use zerocode_core::computer_use_protocol::keys::parse_key_spec;
    for key in ["cmd+v", "command+v", "Ctrl+Shift+T"] {
        let command = zerocode_core::computer_use::parse_command(&words(&[
            "press-key",
            "--app",
            "Fixture",
            "--window-id",
            "41",
            "--background",
            "--key",
            key,
            "--no-screenshot",
        ]))
        .expect("a combination is accepted");
        let mut calls = Vec::new();
        let answer =
            call_with_the_persons_last_step(&command, Asking::HandBack, &mut |method, params| {
                calls.push((
                    method.to_string(),
                    parse_key_spec(params["key"].as_str().unwrap()).unwrap(),
                ));
                Ok(serde_json::json!({"fixture": true}))
            })
            .unwrap();
        assert_eq!(
            calls,
            vec![("hotkey".to_string(), parse_key_spec(key).unwrap())]
        );
        assert_eq!(answer["ranAs"], "hotkey");
        let mut hotkey = command.clone();
        hotkey.method = zerocode_core::computer_use::ComputerMethod::Hotkey;
        let mut hotkey_calls = Vec::new();
        let hotkey_answer =
            call_with_the_persons_last_step(&hotkey, Asking::HandBack, &mut |method, params| {
                assert_eq!(params["app"], "Fixture");
                assert_eq!(params["windowId"], 41);
                assert_eq!(params["background"], true);
                assert_eq!(params["noScreenshot"], true);
                hotkey_calls.push((
                    method.to_string(),
                    parse_key_spec(params["key"].as_str().unwrap()).unwrap(),
                ));
                Ok(serde_json::json!({"fixture": true}))
            })
            .unwrap();
        assert_eq!(calls, hotkey_calls);
        assert!(hotkey_answer.get("ranAs").is_none());
    }
    let command = zerocode_core::computer_use::parse_command(&words(&[
        "press-key",
        "--app",
        "Fixture",
        "--key",
        "Return",
    ]))
    .unwrap();
    let answer = call_with_the_persons_last_step(&command, Asking::HandBack, &mut |method, _| {
        assert_eq!(method, "pressKey");
        Ok(serde_json::json!({"fixture": true}))
    })
    .unwrap();
    assert!(answer.get("ranAs").is_none());
}
