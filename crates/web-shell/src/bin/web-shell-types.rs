fn main() -> std::io::Result<()> {
    let generated = web_shell::protocol::typescript();
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [] => print!("{generated}"),
        [flag, path] if flag == "--write" => std::fs::write(path, generated)?,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "usage: web-shell-types [--write PATH]",
            ));
        }
    }
    Ok(())
}
