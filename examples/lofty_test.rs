use lofty::{read_from_path, TaggedFileExt};
use std::fs;

fn main() {
    // Create a WAV with ffmpeg
    std::process::Command::new("ffmpeg")
        .args(["-v","error","-f","lavfi","-i","sine=frequency=440:duration=1","-q:a","8","-metadata","title=Fixture Track","-y","/tmp/lofty_test.wav"])
        .status().unwrap();
    let data = fs::read("/tmp/lofty_test.wav").unwrap();
    fs::write("/tmp/lofty_test_copy.wav", &data).unwrap();
    let mut tagged = read_from_path("/tmp/lofty_test_copy.wav").unwrap();
    tagged.set_title("Changed Track");
    tagged.save_to_path("/tmp/lofty_test_copy.wav", lofty::config::WriteOptions::default()).unwrap();
    let reloaded = read_from_path("/tmp/lofty_test_copy.wav").unwrap();
    println!("title after: {:?}", reloaded.primary_tag().map(|t| t.title().map(|v| v.into_owned())));
}
