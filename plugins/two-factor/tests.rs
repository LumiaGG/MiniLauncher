use super::*;
use std::fs;

fn test_path(name: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap()
        .join("artifacts/two-factor-tests")
        .join(format!("{name}-{}", crypto::random_id().unwrap()))
        .join("accounts.dat")
}

#[test]
fn rfc6238_all_algorithms_and_timestamps() {
    let times = [
        59,
        1111111109,
        1111111111,
        1234567890,
        2000000000,
        20000000000,
    ];
    let cases = [
        (
            "SHA1",
            "12345678901234567890",
            [
                "94287082", "07081804", "14050471", "89005924", "69279037", "65353130",
            ],
        ),
        (
            "SHA256",
            "12345678901234567890123456789012",
            [
                "46119246", "68084774", "67062674", "91819424", "90698825", "77737706",
            ],
        ),
        (
            "SHA512",
            "1234567890123456789012345678901234567890123456789012345678901234",
            [
                "90693936", "25091201", "99943326", "93441116", "38618901", "47863826",
            ],
        ),
    ];
    for (algorithm, secret, expected) in cases {
        for (time, expected) in times.into_iter().zip(expected) {
            assert_eq!(
                crypto::code(secret.as_bytes(), algorithm, 8, 30, time).unwrap(),
                expected
            );
        }
    }
    assert_eq!(
        crypto::code(b"12345678901234567890", "SHA1", 6, 30, 59).unwrap(),
        "287082"
    );
    assert_eq!(
        crypto::code(b"12345678901234567890", "SHA1", 6, 60, 119).unwrap(),
        "287082"
    );
}

#[test]
fn base32_and_uri_validation() {
    assert_eq!(crypto::base32("mzxw-6===").unwrap(), b"foo");
    assert_eq!(
        crypto::base32("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap(),
        b"12345678901234567890"
    );
    for value in ["", "A", "AB", "MY=", "M=Y", "M0", "MY======X"] {
        assert!(crypto::base32(value).is_err(), "{value}");
    }
    let account = accounts::uri("otpauth://totp/%E9%93%B6%E8%A1%8C:alice%2Btest?secret=MY======&issuer=%E9%93%B6%E8%A1%8C&algorithm=sha256&digits=8&period=60").unwrap();
    assert_eq!(account.name, "alice+test");
    assert_eq!(account.issuer, "银行");
    assert_eq!(account.secret, b"f");
    assert_eq!(
        (account.algorithm.as_str(), account.digits, account.period),
        ("SHA256", 8, 60)
    );
    assert!(account.matches("yh alice"));
    assert!(account.matches("yinhang"));
    for value in [
        "otpauth://hotp/x?secret=MY======",
        "otpauth://totp/x?secret=MY======&period=0",
        "otpauth://totp/x?secret=MY======&secret=MY======",
        "otpauth://totp/a:b?secret=MY======&issuer=c",
        "otpauth://totp/%xx?secret=MY======",
        "otpauth://totp/x?secret=MY======&algorithm=MD5",
        "otpauth://totp/x?secret=MY======&digits=9",
    ] {
        assert!(accounts::uri(value).is_err(), "{value}");
    }
}

#[test]
fn encrypted_store_and_admin_operations() {
    let path = test_path("operations");
    let mut plugin = Plugin { path: path.clone() };
    assert!(plugin.query("", 100).unwrap()[0]["title"].as_str().unwrap().contains("尚无"));
    assert!(plugin.execute("hint").is_err());
    let call = |request: Value| admin::handle(&path, &request);
    assert!(!path.exists(), "listing must not create accounts");
    call(json!({"op":"list"})).unwrap();
    assert!(!path.exists());
    let reply = call(json!({"op":"add","name":"银行","secret":"JBSWY3DPEHPK3PXP","issuer":"Bank"})).unwrap();
    assert!(reply.to_string().find("JBSWY3DPEHPK3PXP").is_none(), "secrets are never returned");
    call(json!({"op":"add","name":"邮箱","secret":"GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ","issuer":"Mail","algorithm":"sha256","digits":8,"period":60})).unwrap();
    let account = accounts::load(&path).unwrap().remove(0);
    assert_eq!(account.name, "银行");
    let bytes = fs::read(&path).unwrap();
    assert!(!bytes.windows(account.secret.len()).any(|s| s == account.secret));
    let copy = plugin.execute(&format!("copy:{}", account.id)).unwrap();
    assert_eq!(copy["action"]["text"].as_str().unwrap().len(), 6);
    let second = accounts::load(&path).unwrap().remove(1);
    assert_eq!((second.algorithm.as_str(), second.digits, second.period), ("SHA256", 8, 60));
    // Duplicate add leaves the two existing accounts intact.
    let duplicate = call(json!({"op":"add","name":"银行","secret":"JBSWY3DPEHPK3PXP","issuer":"Bank"})).unwrap();
    assert_eq!(duplicate["accounts"].as_array().unwrap().len(), 2);
    call(json!({"op":"uri","text":"otpauth://totp/Svc:me?secret=MY======&issuer=Svc"})).unwrap();
    assert_eq!(accounts::load(&path).unwrap().len(), 3);
    call(json!({"op":"edit","id":account.id,"name":"银行","issuer":"Bank","remark":"我的银行"})).unwrap();
    assert_eq!(accounts::load(&path).unwrap()[0].remark, "我的银行");
    assert!(call(json!({"op":"edit","id":account.id,"name":"","issuer":"","remark":""})).is_err());
    assert!(call(json!({"op":"add","name":"x","secret":"MY======","digits":9})).is_err());
    assert!(call(json!({"op":"delete","id":"missing"})).is_err());
    call(json!({"op":"delete","id":account.id})).unwrap();
    assert!(plugin.execute(&format!("copy:{}", account.id)).is_err());
    assert_eq!(accounts::load(&path).unwrap().len(), 2);
    fs::write(&path, b"broken data").unwrap();
    assert!(plugin.query("", 100).is_err());
    assert!(call(json!({"op":"add","name":"x","secret":"MY======"})).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"broken data", "never replace unreadable accounts");
}
#[test]
fn flow_json_import_preserves_metadata_and_validates_whole_batch() {
    let path = test_path("import");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let entry = json!({"OtpType":"totp","Name":"alice","Issuer":"Bank","Remark":"工作账号","Secret":"JBSWY3DPEHPK3PXP","Algorithm":"SHA512","Digits":8,"Counter":0});
    for json in [json!([entry.clone()]), json!({"OtpParams":[entry.clone()]})] {
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let imported = accounts::import_file(&path).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].remark, "工作账号");
        assert_eq!(imported[0].algorithm, "SHA512");
        assert_eq!(imported[0].digits, 8);
    }
    let mut invalid = entry.clone();
    invalid["Digits"] = json!(4294967302u64);
    fs::write(&path, serde_json::to_vec(&json!([entry, invalid])).unwrap()).unwrap();
    assert!(accounts::import_file(&path).is_err());
    fs::write(
        &path,
        "otpauth://totp/test?secret=MY======\notpauth://hotp/bad?secret=MY======",
    )
    .unwrap();
    assert!(accounts::import_file(&path).is_err());
}
