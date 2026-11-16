use super::*;
struct Fixture {
    root: PathBuf,
    store: Option<Store>,
}
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!("piin-test-{}", random_id().unwrap()));
        let store = Some(Store::open(root.clone()).unwrap());
        Self { root, store }
    }
    fn store(&mut self) -> &mut Store {
        self.store.as_mut().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.take();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn save_action(store: &mut Store, text: &str) -> String {
    let result = store.query(&json!({"query":text})).unwrap();
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|i| {
            i["action"]["id"]
                .as_str()
                .filter(|id| id.starts_with("save:"))
                .map(str::to_owned)
        })
        .unwrap()
}
#[test]
fn saves_only_on_execute_and_preserves_pins_across_restart() {
    let mut fixture = Fixture::new();
    let action = save_action(fixture.store(), "第一条备注\nsecond line");
    assert!(fixture.store().notes.is_empty());
    assert!(!fixture.root.join("notes.json").exists());
    fixture.store().execute(&action).unwrap();
    assert!(fixture.store().execute(&action).is_err());
    let id = fixture.store().notes[0].id.clone();
    fixture.store().execute(&format!("pin:{id}")).unwrap();
    let action = save_action(fixture.store(), "other note");
    fixture.store().execute(&action).unwrap();
    let results = fixture.store().query(&json!({"query":""})).unwrap();
    assert_eq!(results["items"][0]["id"], id);
    fixture.store.take();
    fixture.store = Some(Store::open(fixture.root.clone()).unwrap());
    assert_eq!(fixture.store().notes.len(), 2);
    let drag = fixture.store().prepare_drag(&id).unwrap();
    assert_eq!(drag["data"]["text"], "第一条备注\nsecond line");
    assert!(
        fixture
            .store()
            .notes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .pinned
    );
}
#[test]
fn image_draft_is_independent_of_source_and_delete_removes_both_files() {
    let mut fixture = Fixture::new();
    let source = fixture.root.join("input.png");
    Image {
        width: 2,
        height: 2,
        pixels: vec![255; 16],
    }
    .write_png(&source)
    .unwrap();
    let result = fixture
        .store()
        .query(&json!({"query":"示意图","attachments":[{"type":"image","path":source}]}))
        .unwrap();
    let action = result["items"][0]["action"]["id"].as_str().unwrap();
    fs::remove_file(source).unwrap();
    fixture.store().execute(action).unwrap();
    let id = fixture.store().notes[0].id.clone();
    assert_eq!(
        fixture.store().execute(&format!("text:{id}")).unwrap()["action"]["text"],
        "示意图"
    );
    let original = fixture.store().image_path(&id, false);
    let thumb = fixture.store().image_path(&id, true);
    assert!(original.exists() && thumb.exists());
    assert_eq!(
        clipboard_image::decode(&fs::read(&original).unwrap(), false)
            .unwrap()
            .width,
        2
    );
    assert_eq!(
        fixture.store().prepare_drag(&id).unwrap()["data"]["path"],
        json!(original)
    );
    fixture.store().execute(&format!("delete:{id}")).unwrap();
    assert!(!original.exists() && !thumb.exists());
    assert!(fixture.store().notes.is_empty());
}
#[test]
fn stale_save_and_capacity_failure_do_not_overwrite_saved_notes() {
    let mut fixture = Fixture::new();
    let stale = save_action(fixture.store(), "old draft");
    let latest = save_action(fixture.store(), "latest draft");
    assert!(fixture.store().execute(&stale).is_err());
    fixture.store().execute(&latest).unwrap();
    let before = fs::read(fixture.root.join("notes.json")).unwrap();
    let mut oversized = fixture.store().notes.clone();
    oversized[0].text = "x".repeat(MAX_TEXT + 1);
    assert!(fixture.store().commit(oversized).is_err());
    assert_eq!(fixture.store().notes[0].text, "latest draft");
    assert_eq!(fs::read(fixture.root.join("notes.json")).unwrap(), before);
    let note = fixture.store().notes[0].clone();
    let full: Vec<Note> = (0..MAX_NOTES)
        .map(|_| Note {
            id: random_id().unwrap(),
            ..note.clone()
        })
        .collect();
    fixture.store().commit(full).unwrap();
    let action = save_action(fixture.store(), "one too many");
    assert!(fixture.store().execute(&action).is_err());
    assert_eq!(fixture.store().notes.len(), MAX_NOTES);
}
