//! Minimal Rust client for TypeScript 7's `tsc --api` (msgpack-framed, JSON payload) protocol.
//! Frame: fixarray(3) [ type:u8, method:bin, payload:bin ]; 1=request 4=response 5=error 6=callback.
//! usage: tsgo-api-client <tsc-binary> <root> <tsconfig> <file> <utf16-offset>
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

fn write_bin(buf: &mut Vec<u8>, b: &[u8]) {
    let n = b.len();
    if n < 256 { buf.push(0xc4); buf.push(n as u8); }
    else if n < 65536 { buf.push(0xc5); buf.extend_from_slice(&(n as u16).to_be_bytes()); }
    else { buf.push(0xc6); buf.extend_from_slice(&(n as u32).to_be_bytes()); }
    buf.extend_from_slice(b);
}
fn read_bin(r: &mut impl Read) -> Vec<u8> {
    let mut t = [0u8; 1]; r.read_exact(&mut t).unwrap();
    let n = match t[0] {
        0xc4 => { let mut b = [0u8; 1]; r.read_exact(&mut b).unwrap(); b[0] as usize }
        0xc5 => { let mut b = [0u8; 2]; r.read_exact(&mut b).unwrap(); u16::from_be_bytes(b) as usize }
        0xc6 => { let mut b = [0u8; 4]; r.read_exact(&mut b).unwrap(); u32::from_be_bytes(b) as usize }
        x => panic!("unexpected bin tag {x:#x}"),
    };
    let mut v = vec![0u8; n]; r.read_exact(&mut v).unwrap(); v
}
struct Api { stdin: std::process::ChildStdin, stdout: std::process::ChildStdout }
impl Api {
    fn req(&mut self, method: &str, payload: &str) -> String {
        let mut buf = vec![0x93, 1];
        write_bin(&mut buf, method.as_bytes());
        write_bin(&mut buf, payload.as_bytes());
        self.stdin.write_all(&buf).unwrap(); self.stdin.flush().unwrap();
        let mut h = [0u8; 2]; self.stdout.read_exact(&mut h).unwrap();
        assert_eq!(h[0], 0x93);
        let _name = read_bin(&mut self.stdout);
        let body = String::from_utf8(read_bin(&mut self.stdout)).unwrap();
        match h[1] { 4 => body, 5 => format!("ERROR: {body}"), t => panic!("msg type {t} (callbacks not implemented)") }
    }
}
// crude field extraction to stay dependency-free
fn field<'a>(json: &'a str, key: &str) -> &'a str {
    let k = format!("\"{key}\":"); let i = json.find(&k).expect(key) + k.len();
    let rest = &json[i..]; let end = rest.find(|c| c == ',' || c == '}').unwrap();
    rest[..end].trim_matches('"')
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (exe, root, tsconfig, file, off) = (&a[1], &a[2], &a[3], &a[4], &a[5]);
    let t0 = Instant::now();
    let mut child = Command::new(exe).args(["--api", "--cwd", root]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut api = Api { stdin: child.stdin.take().unwrap(), stdout: child.stdout.take().unwrap() };
    println!("initialize: {}", api.req("initialize", "null"));
    let snap = api.req("updateSnapshot", &format!(r#"{{"openProjects":["{tsconfig}"]}}"#));
    let (sid, pid) = (field(&snap, "snapshot").to_string(), field(&snap, "id").to_string());
    println!("snapshot={sid} project={pid} ({:?})", t0.elapsed());
    let t1 = Instant::now();
    let sym = api.req("getSymbolAtPosition", &format!(r#"{{"snapshot":{sid},"project":"{pid}","file":"{file}","position":{off}}}"#));
    println!("symbol: {} ({:?})", &sym[..sym.len().min(200)], t1.elapsed());
    let ty = api.req("getTypeAtPosition", &format!(r#"{{"snapshot":{sid},"project":"{pid}","file":"{file}","position":{off}}}"#));
    let tid = field(&ty, "id").to_string();
    println!("typeToString: {} (total {:?})", api.req("typeToString", &format!(r#"{{"snapshot":{sid},"project":"{pid}","type":{tid}}}"#)), t0.elapsed());
    child.kill().ok();
}
