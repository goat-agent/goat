use std::io::{self, BufRead, Write};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;

pub(crate) fn say(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    writeln!(out, "{text}")?;
    out.flush()
}

pub(crate) fn line(prompt: &str) -> io::Result<String> {
    let mut out = io::stdout();
    write!(out, "{prompt}")?;
    out.flush()?;
    let mut answer = String::new();
    if io::stdin().lock().read_line(&mut answer)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "input closed"));
    }
    Ok(answer.trim().to_owned())
}

pub(crate) fn choose(title: &str, options: &[&str]) -> io::Result<usize> {
    say(title)?;
    for (index, option) in options.iter().enumerate() {
        say(&format!("  {}. {option}", index + 1))?;
    }
    loop {
        let answer = line("› ")?;
        if let Some(index) = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|i| *i < options.len())
        {
            return Ok(index);
        }
        say(&format!("Enter a number from 1 to {}.", options.len()))?;
    }
}

pub(crate) fn secret(prompt: &str) -> io::Result<String> {
    let mut out = io::stdout();
    terminal::enable_raw_mode()?;
    let result = write!(out, "{prompt}")
        .and_then(|()| out.flush())
        .and_then(|()| read_hidden());
    terminal::disable_raw_mode()?;
    writeln!(out)?;
    result
}

fn read_hidden() -> io::Result<String> {
    let mut value = String::new();
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return Ok(value.trim().to_owned()),
            KeyCode::Char('j' | 'm') if ctrl => return Ok(value.trim().to_owned()),
            KeyCode::Char('c') if ctrl => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            KeyCode::Char(c) if !ctrl => value.push(c),
            KeyCode::Backspace => {
                value.pop();
            }
            _ => {}
        }
    }
}
