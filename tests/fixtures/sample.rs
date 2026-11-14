// Sample Rust file for highlighting.
use std::collections::HashMap;

/// A documented struct.
#[derive(Debug, Clone)]
pub struct Point {
	x: f64, // tab indented
    y: f64,
}

fn main() {
    let mut m: HashMap<&str, i32> = HashMap::new();
    m.insert("answer", 42);
    let s = "a string with \"escapes\"";
    if m["answer"] > 10 && !s.is_empty() {
        println!("{} {}", m["answer"], s);
    }
}
