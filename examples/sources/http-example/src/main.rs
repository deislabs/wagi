fn main() {
    let url = "https://api.brigade.sh/healthz".to_string();
    let req = http::request::Builder::new().uri(&url).body(None).unwrap();
    let mut res = wasi_experimental_http::request(req).expect("cannot make get request");

    let header_map = res.headers_get_all().unwrap();
    let content_type = res.header_get("content-type".to_string()).unwrap();

    let str = std::str::from_utf8(&res.body_read_all().unwrap())
        .unwrap()
        .to_string();

    println!("Content-Type: text/plain\n");

    if res.status_code == 200 {
        println!("api.brigade.sh is HEALTHY");
        println!("The health check response had {} header(s)", header_map.len());
        println!("Its content type was: {}", content_type);
        println!("Its body content was: {}", str);
    } else {
        println!("api.brigade.sh is UNHEALTHY");
        println!("The health check status code was: {}", res.status_code);
        println!("The response body was: {}", str);
    }
}
