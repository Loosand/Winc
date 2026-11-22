use super::*;

fn sample_folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("minc-{}-{name}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    dir
}

#[test]
fn both_syntaxes_share_a_target_but_keep_separate_citations() {
    let dir = sample_folder("note-link-index");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let source = root.join("A.md");
    let target = root.join("音乐审美？.md");
    std::fs::write(&target, "# 音乐审美\n").unwrap();
    std::fs::write(
        &source,
        "关于 [[音乐审美？|审美]] 和 [同一个目标](./音乐审美？.md)。\n\n`[[音乐审美？]]`\n",
    )
    .unwrap();
    let first = update(&root, None, Changes::All);
    let backlinks = first.backlinks(&target).unwrap();
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[&source].len(), 2);
    assert_eq!(backlinks[&source][0].label, "审美");
    assert_eq!(first.graph.edges, [(0, 1)]);

    std::fs::write(&source, "[[音乐审美？]]").unwrap();
    let second = update(&root, Some(&first), Changes::Paths([source.clone()].into()));
    assert_eq!(second.backlinks(&target).unwrap()[&source].len(), 1);
    assert_eq!(second.graph.edges, [(0, 1)]);
    assert!(Arc::ptr_eq(&first.graph, &second.graph));
    assert!(Arc::ptr_eq(&first.notes[&target], &second.notes[&target]));
    std::fs::write(&source, "没有引用。").unwrap();
    let third = update(&root, Some(&second), Changes::Paths([source].into()));
    assert!(third.backlinks(&target).is_none());
    assert!(third.graph.edges.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn url_fragments_and_escapes_are_distinct_from_filename_punctuation() {
    let dir = sample_folder("note-link-escapes");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let source = root.join("A.md");
    let target = root.join("notes/章 节#.md");
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(&target, "# 中文标题").unwrap();
    std::fs::write(&source, "[# 标题](#标题) [**中文**](./notes/章%20节%23.md#中文标题) [外链](https://example.org) ![附件](./notes/章%20节%23.md)\n[[../outside]]").unwrap();
    let index = update(&root, None, Changes::All);
    let backlinks = &index.backlinks(&target).unwrap()[&source];
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].label, "中文");
    assert_eq!(index.graph.edges.len(), 1);
    assert!(index.backlinks(&source).is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wiki_links_in_the_original_and_its_copy_use_the_nearest_directory() {
    let dir = sample_folder("note-link-nearest");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let copy = root.join("Export/Notebook");
    let source = root.join("Thoughts/from.md");
    let target = root.join("Topics/same.md");
    let copied_source = copy.join("Thoughts/from.md");
    let copied_target = copy.join("Topics/same.md");
    let same_directory = root.join("Thoughts/same.md");
    for note in [&source, &target, &copied_source, &copied_target] {
        std::fs::create_dir_all(note.parent().unwrap()).unwrap();
    }
    for note in [&source, &copied_source] {
        std::fs::write(note, "[[same]] [[same.md]]").unwrap();
    }
    for note in [&target, &copied_target] {
        std::fs::write(note, "# Note").unwrap();
    }

    let index = update(&root, None, Changes::All);
    assert_eq!(
        index.resolve_wiki(&root, &source, "same"),
        WikiResult::Found(&target)
    );
    assert_eq!(
        index.resolve_wiki(&root, &copied_source, "same"),
        WikiResult::Found(&copied_target)
    );
    assert_eq!(
        index.resolve_wiki(&root, &source, "Export/Notebook/Topics/same"),
        WikiResult::Found(&copied_target)
    );
    assert_eq!(
        index.resolve_wiki(&root, &copied_source, "../Topics/same"),
        WikiResult::Found(&copied_target)
    );
    assert_eq!(index.backlinks(&target).unwrap()[&source].len(), 2);
    assert_eq!(
        index.backlinks(&copied_target).unwrap()[&copied_source].len(),
        2
    );
    let edges: BTreeSet<_> = index
        .graph
        .edges
        .iter()
        .map(|&(from, to)| (&index.graph.paths[from], &index.graph.paths[to]))
        .collect();
    assert_eq!(
        edges,
        BTreeSet::from([(&source, &target), (&copied_source, &copied_target)])
    );

    std::fs::write(&same_directory, "# Nearer note").unwrap();
    let updated = update(
        &root,
        Some(&index),
        Changes::Paths([same_directory.clone()].into()),
    );
    assert_eq!(
        updated.resolve_wiki(&root, &source, "same"),
        WikiResult::Found(&same_directory)
    );
    assert_eq!(
        updated.backlinks(&same_directory).unwrap()[&source].len(),
        2
    );
    assert!(updated.backlinks(&target).is_none());
    assert_eq!(
        updated.backlinks(&copied_target).unwrap()[&copied_source].len(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wiki_path_rebinds_when_an_equally_near_duplicate_appears_and_disappears() {
    let dir = sample_folder("note-link-ambiguity");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let notes = root.join("notes");
    let source = root.join("from.md");
    let original = root.join("archive/same.md");
    let duplicate = notes.join("same.md");
    std::fs::create_dir_all(original.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(&original, "# Note").unwrap();
    std::fs::write(
        &source,
        "[[same]] [[notes/same#Note]] [website](https://example.org)",
    )
    .unwrap();
    let first = update(&root, None, Changes::All);
    assert_eq!(
        first.resolve_wiki(&root, &source, "same"),
        WikiResult::Found(&original)
    );
    std::fs::write(&duplicate, "# Note").unwrap();
    let second = update(
        &root,
        Some(&first),
        Changes::Paths([duplicate.clone()].into()),
    );
    assert!(matches!(
        second.resolve_wiki(&root, &source, "same"),
        WikiResult::Ambiguous(paths) if paths.len() == 2
    ));
    assert_eq!(
        second.resolve_wiki(&root, &source, "./archive/same"),
        WikiResult::Found(&original)
    );
    assert!(second.backlinks(&original).is_none());
    assert_eq!(second.backlinks(&duplicate).unwrap()[&source].len(), 1);
    std::fs::remove_dir_all(&notes).unwrap();
    let third = update(&root, Some(&second), Changes::Paths([notes].into()));
    assert_eq!(
        third.resolve_wiki(&root, &source, "same"),
        WikiResult::Found(&original)
    );
    assert!(third.backlinks(&duplicate).is_none());
    assert_eq!(third.backlinks(&original).unwrap()[&source].len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}
