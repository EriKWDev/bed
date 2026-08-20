pub fn resource_directory(name: &str) -> String {
    resolve(name)
}

pub fn writable_data_directory(name: &str) -> String {
    resolve(name)
}

fn resolve(name: &str) -> String {
    std::env::current_dir()
        .map(|directory| directory.join(name).to_string_lossy().into_owned())
        .unwrap_or_else(|_| name.to_string())
}
