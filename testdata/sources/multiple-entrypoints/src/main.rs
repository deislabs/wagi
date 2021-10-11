fn main() {
  println!("Content-Type: text/plain\n");
  println!("Default entrypoint");
}

#[no_mangle]
pub fn ep1() {
    println!("Content-Type: text/plain\n");
    println!("Entrypoint 1");
}

#[no_mangle]
pub fn ep2() {
    println!("Content-Type: text/plain\n");
    println!("Entrypoint 2");
}
