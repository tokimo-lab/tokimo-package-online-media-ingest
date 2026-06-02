/// Sanitize a string for use as a filename, removing or replacing unsafe characters.
pub fn sanitize_filename(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    let trimmed = sanitized.trim().trim_matches('.');
    if trimmed.is_empty() {
        return "download".to_string();
    }

    // Collapse consecutive underscores
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_underscore = false;
    for c in trimmed.chars() {
        if c == '_' {
            if !prev_underscore {
                result.push(c);
            }
            prev_underscore = true;
        } else {
            result.push(c);
            prev_underscore = false;
        }
    }

    // Truncate to 200 chars to avoid filesystem limits
    if result.len() > 200 {
        result.truncate(200);
        while !result.is_char_boundary(result.len()) {
            result.pop();
        }
    }

    result
}
