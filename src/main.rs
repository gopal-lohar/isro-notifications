mod scrapper;

fn main() {
    println!("Hello, world!");
    let now = chrono::Utc::now().format("%Y-%m-%d-%H-%M-%S");
    println!("{now}");
}
