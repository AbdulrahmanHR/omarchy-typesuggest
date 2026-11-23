use std::time::Instant;
use typesuggest::dict::{Context, Dictionary};
fn main() {
    let mut dict = Dictionary::embedded();
    dict.set_typo_mode(&std::env::args().nth(1).unwrap_or_default());
    let ctx = Context::default();
    for q in ["teh", "exmap", "definately", "recieved", "xzqwv", "seperate", "tommorow", "fucnti", "goverm"] {
        let t = Instant::now();
        let mut r = vec![];
        for _ in 0..200 {
            r = dict.suggest(q, &ctx, 3);
        }
        println!("{q:<12} {:>7.0} us  {:?}", t.elapsed().as_secs_f64() * 1e6 / 200.0, r);
    }
}
