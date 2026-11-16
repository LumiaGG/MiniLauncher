#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryIntent {
    /// Explicit port query, e.g. ":3000", "port 8080", "port:80"
    Port(u16),
    /// Bare "port" - list all listening/active ports
    AllPorts,
    /// Explicit kill command, e.g. "kill node", "kill 14208"
    Kill(String),
    /// Explicit ps command, e.g. "ps chrome", "ps 1234"
    Process(String),
    /// Bare "ps" or "kill" or empty query -> overview of top memory-consuming processes
    TopProcesses,
    /// General search (matching process name, PID, or potential port)
    General(String),
}

pub fn parse_query(raw: &str) -> QueryIntent {
    let text = raw.trim();

    if text.is_empty() {
        return QueryIntent::TopProcesses;
    }

    // 1. Direct prefix ':' e.g. ":8080" or ": 8080"
    if let Some(rest) = text.strip_prefix(':') {
        let rest = rest.trim();
        if let Ok(port) = rest.parse::<u16>() {
            return QueryIntent::Port(port);
        }
        if rest.is_empty() {
            return QueryIntent::AllPorts;
        }
    }

    // 2. Keyword "port"
    if text.eq_ignore_ascii_case("port") {
        return QueryIntent::AllPorts;
    }
    if let Some(rest) = strip_keyword_ignore_ascii_case(text, "port") {
        let rest = rest.trim().trim_start_matches(':').trim();
        if let Ok(port) = rest.parse::<u16>() {
            return QueryIntent::Port(port);
        }
        if rest.is_empty() {
            return QueryIntent::AllPorts;
        }
        return QueryIntent::General(rest.to_string());
    }

    // 3. Keyword "kill"
    if text.eq_ignore_ascii_case("kill") {
        return QueryIntent::TopProcesses;
    }
    if let Some(rest) = strip_keyword_ignore_ascii_case(text, "kill") {
        let rest = rest.trim();
        if rest.is_empty() {
            return QueryIntent::TopProcesses;
        }
        return QueryIntent::Kill(rest.to_string());
    }

    // 4. Keyword "ps"
    if text.eq_ignore_ascii_case("ps") {
        return QueryIntent::TopProcesses;
    }
    if let Some(rest) = strip_keyword_ignore_ascii_case(text, "ps") {
        let rest = rest.trim();
        if rest.is_empty() {
            return QueryIntent::TopProcesses;
        }
        return QueryIntent::Process(rest.to_string());
    }

    // Bare numbers may identify a PID as well as a port.
    QueryIntent::General(text.to_string())
}

fn strip_keyword_ignore_ascii_case<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    if text.len() >= keyword.len()
        && text[..keyword.len()].eq_ignore_ascii_case(keyword)
    {
        let rest = &text[keyword.len()..];
        if rest.is_empty()
            || rest.starts_with(char::is_whitespace)
            || rest.starts_with(':')
        {
            return Some(rest);
        }
    }
    None
}
