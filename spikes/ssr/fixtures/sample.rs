fn main() {
    let a = foo().unwrap();
    let b = a.get(0).unwrap().bar.unwrap(); // nested
    let f = File::open("x").unwrap();
    let s = x
        .iter()
        .map(|v| v + 1)
        .collect::<Vec<_>>()
        .first()
        .unwrap();
    match kind {
        // leading comment
        Kind::A => 1,
        /* block */ Kind::B => {
            2
        }
        _ => 3, // trailing
    }
}
