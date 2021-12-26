use ariadne::FnCache;

use crate::Error;

use std::fmt::Debug;

use super::mirrors::{MirrorLabel, MirrorReport};

#[allow(clippy::ptr_arg)]
fn provider(x: &String) -> Result<String, Box<(dyn Debug + 'static)>> {
    if x == "<internal>" || x == "<native>" {
        Ok("Error originated from native context".to_owned())
    } else {
        std::fs::read_to_string(x).map_err(|x| Box::new(x) as Box<(dyn Debug + 'static)>)
    }
}

pub fn report(mut e: Error) {
    let mirror = MirrorReport::from(&mut e);
    let file_name = mirror.location.0.clone();
    if file_name == "<internal>" || file_name == "<native>" {
        mirror.location.1 = 0;
        mirror.labels.iter_mut().for_each(|l| {
            MirrorLabel::from(l).span.1 = 0..("Error originated from native context".len());
        });
    }
    e.print(FnCache::new(provider)).unwrap();
}
