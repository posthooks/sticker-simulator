/// Requests a string from the user with the specified prompt.
pub fn get_string(prompt: &str) -> String {
    get_input(prompt, false).unwrap_or_default()
}

/// Requests a string from the user with the specified prompt, treating the input as a password.
pub fn get_password(prompt: &str) -> String {
    get_input(prompt, true).unwrap_or_default()
}

fn get_input(prompt: &str, is_password: bool) -> Option<String> {
    if is_password {
        println!("{}:{}", G