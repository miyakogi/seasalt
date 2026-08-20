pub fn bash_init_script() -> &'static str {
    include_str!("bash/seasalt.bash")
}

pub fn zsh_init_script() -> &'static str {
    include_str!("zsh/seasalt.zsh")
}
