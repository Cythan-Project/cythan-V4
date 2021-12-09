use std::ops::Range;

use ariadne::{Report, Source};

use super::mirrors::{MirrorLabel, MirrorReport};

pub fn report(mut e: Report<(String, Range<usize>)>) {
    let mirror = MirrorReport::from(&mut e);
    let file_name = mirror.location.0.clone();
    if file_name == "<internal>" || file_name == "<native>" {
        mirror.location.1 = 0;
        mirror.labels.iter_mut().for_each(|l| {
            MirrorLabel::from(l).span.1 = 0..("Error originated from native context".len());
        });
        e.print((
            file_name.clone(),
            Source::from("Error originated from native context".to_owned()),
        ))
        .unwrap();
    } else {
        e.eprint((
            file_name.clone(),
            Source::from(std::fs::read_to_string(&file_name).unwrap()),
        ))
        .unwrap();
    }
}
