mod common;
use common::{tempdir, Fixture};
use git_wrapped::{analysis::analyze, config::Config, git::discover, render::Theme};
use image::{codecs::gif::GifDecoder, AnimationDecoder};
use std::fs;

fn frames(path: &std::path::Path) -> Vec<image::Frame> {
    let decoder = GifDecoder::new(std::io::BufReader::new(fs::File::open(path).unwrap())).unwrap();
    decoder.into_frames().collect_frames().unwrap()
}

fn two_commit_data() -> git_wrapped::model::RepositoryAnalytics {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-02T00:30:00 +1400");
    f.commit("b", b"b\n", "a@x", "2024-01-01T23:30:00 -1200");
    analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap()
}

#[test]
fn gif_story_has_fixed_frames_and_delays() {
    let data = two_commit_data();
    let dir = tempdir();
    let out = dir.path().join("story.gif");
    git_wrapped::motion::write_gif(&data, &out, Theme::Dark).unwrap();
    let bytes = fs::read(&out).unwrap();
    assert_eq!(&bytes[..6], b"GIF89a");
    assert!(bytes.len() < 20_000_000);
    let frames = frames(&out);
    assert_eq!(frames.len(), 49);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.buffer().dimensions(), (720, 720));
        let (numer, denom) = frame.delay().numer_denom_ms();
        let expected = if index % 7 == 6 { 1800 } else { 100 };
        assert_eq!(numer / denom, expected, "frame {index}");
    }
    let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1);
}

#[test]
fn gif_story_is_complete_and_deterministic_for_one_commit() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let dir = tempdir();
    let first = dir.path().join("one.gif");
    let second = dir.path().join("two.gif");
    git_wrapped::motion::write_gif(&data, &first, Theme::Light).unwrap();
    git_wrapped::motion::write_gif(&data, &second, Theme::Light).unwrap();
    assert_eq!(frames(&first).len(), 49);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
}

#[test]
fn story_scenes_escape_hostile_text() {
    let mut data = two_commit_data();
    data.repository.name = "<svg onload=x>&\"".into();
    data.contributors[0].name = "</text><script>".into();
    data.files[0].display_path = "a<b>|c\n.rs".into();
    let scenes = git_wrapped::motion::story::scenes(&data, Theme::Dark);
    assert_eq!(scenes.len(), 7);
    for scene in &scenes {
        for index in 0..7 {
            let svg = scene.at(index, 7);
            assert!(!svg.contains("<script") && !svg.contains("<svg onload"));
            assert_eq!(svg.matches("<svg").count(), 1);
            git_wrapped::render::raster::render_rgba(&svg, 720, 720).unwrap();
        }
    }
}

#[test]
fn gif_refuses_symlink_output_and_keeps_target() {
    let data = two_commit_data();
    let dir = tempdir();
    let target = dir.path().join("target.txt");
    fs::write(&target, "untouched").unwrap();
    let link = dir.path().join("story.gif");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = git_wrapped::motion::write_gif(&data, &link, Theme::Dark).unwrap_err();
    assert!(error.contains("symlink"), "{error}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}
