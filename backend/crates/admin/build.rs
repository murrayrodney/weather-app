// `sqlx::migrate!` embeds backend/migrations at compile time; rebuild when a migration changes.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
