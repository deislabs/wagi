fn main() {
    println!("Content-Type: text/plain\n");
    println!("This is the main entry point");
    print_env();
}

#[no_mangle]
pub fn on_exact() {
    println!("Content-Type: text/plain; charset=UTF-8\n");
    println!("This is the .../exact handler");
    print_env();
}

#[no_mangle]
pub fn on_wildcard() {
    println!("Content-Type: text/plain; charset=UTF-8\n");
    println!("This is the .../wildcard/... handler");
    print_env();
}

#[no_mangle]
pub fn _routes() {
    println!("/exact on_exact");
    println!("/wildcard/... on_wildcard");
    println!("/main _start");
}

fn print_env() {
    let mut env_vars: Vec<_> = std::env::vars().collect();
    env_vars.sort();
    for (k, v) in env_vars {
        println!("{} = {}", k, v);
    }
}
