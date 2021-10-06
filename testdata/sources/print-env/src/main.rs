fn main() {
    println!("Content-Type: text/plain\n");
    let mut env_vars: Vec<_> = std::env::vars().collect();
    env_vars.sort();
    for (k, v) in env_vars {
        println!("{} = {}", k, v);
    }
}
