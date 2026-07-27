//! Shell completion scripts, generated from the registry.
//!
//! These are static: a shell cannot ask the device what its channels are called
//! without opening it, which would be rude in a completion hook. So the scripts
//! cover parameter paths, verbs and enum values, which is the bulk of the
//! benefit, and leave channel names to the interactive completer where a session
//! is already open.

use dspi_proto::registry::{Kind, REGISTRY};

use crate::VERBS;

/// Every parameter path, sorted.
fn paths() -> Vec<&'static str> {
    let mut v: Vec<&str> = REGISTRY
        .iter()
        .filter(|d| d.set.is_some() || d.get.is_some())
        .map(|d| d.path)
        .collect();
    v.sort_unstable();
    v
}

fn verbs() -> Vec<&'static str> {
    VERBS.iter().map(|(n, _)| *n).collect()
}

/// Paths whose value is a fixed set, with the words that are legal there.
fn enum_paths() -> Vec<(&'static str, Vec<&'static str>)> {
    REGISTRY
        .iter()
        .filter_map(|d| match d.kind {
            Kind::Bool => Some((d.path, vec!["on", "off"])),
            Kind::Choice(v) if !v.is_empty() => Some((d.path, v.iter().map(|(_, n)| *n).collect())),
            _ => None,
        })
        .collect()
}

pub fn generate(shell: &str) -> Option<String> {
    Some(match shell {
        "bash" => bash(),
        "zsh" => zsh(),
        "fish" => fish(),
        "powershell" | "pwsh" => powershell(),
        _ => return None,
    })
}

pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

fn bash() -> String {
    let paths = paths().join(" ");
    let verbs = verbs().join(" ");

    let mut cases = String::new();
    for (path, values) in enum_paths() {
        cases.push_str(&format!(
            "        {})\n            COMPREPLY=($(compgen -W \"{}\" -- \"$cur\"))\n            return\n            ;;\n",
            path,
            values.join(" ")
        ));
    }

    format!(
        r#"# dspi completion for bash. Generated; do not edit.
# Install:  dspi completions bash > /etc/bash_completion.d/dspi

_dspi() {{
    local cur prev
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"

    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=($(compgen -W "{verbs} {paths}" -- "$cur"))
        return
    fi

    case "$prev" in
        get|set)
            COMPREPLY=($(compgen -W "{paths}" -- "$cur"))
            return
            ;;
        completions)
            COMPREPLY=($(compgen -W "bash zsh fish powershell" -- "$cur"))
            return
            ;;
        --device)
            # Serial numbers of anything currently attached.
            COMPREPLY=($(compgen -W "$(dspi list 2>/dev/null | awk '{{print $1}}')" -- "$cur"))
            return
            ;;
{cases}    esac

    COMPREPLY=($(compgen -W "--json --quiet --dry-run --device" -- "$cur"))
}}

complete -F _dspi dspi
"#
    )
}

fn zsh() -> String {
    // Descriptions are worth carrying here: zsh shows them inline, which turns
    // completion into documentation.
    let mut descs = String::new();
    for d in REGISTRY
        .iter()
        .filter(|d| d.set.is_some() || d.get.is_some())
    {
        let plain = d.plain.replace('\'', "").replace(':', " -");
        descs.push_str(&format!("        '{}:{}'\n", d.path, plain));
    }

    let mut verb_lines = String::new();
    for (n, d) in VERBS {
        verb_lines.push_str(&format!("        '{n}:{d}'\n"));
    }

    let mut cases = String::new();
    for (path, values) in enum_paths() {
        cases.push_str(&format!(
            "            {})\n                _values 'value' {}\n                ;;\n",
            path,
            values
                .iter()
                .map(|v| format!("'{v}'"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    format!(
        r#"#compdef dspi
# dspi completion for zsh. Generated; do not edit.
# Install:  dspi completions zsh > "${{fpath[1]}}/_dspi"

_dspi() {{
    local -a params verbs
    params=(
{descs}    )
    verbs=(
{verb_lines}    )

    if (( CURRENT == 2 )); then
        _describe -t commands 'command' verbs
        _describe -t parameters 'parameter' params
        return
    fi

    case "${{words[CURRENT-1]}}" in
        get|set)
            _describe -t parameters 'parameter' params
            return
            ;;
        completions)
            _values 'shell' 'bash' 'zsh' 'fish' 'powershell'
            return
            ;;
        --device)
            local -a serials
            serials=(${{(f)"$(dspi list 2>/dev/null | awk '{{print $1}}')"}})
            _describe -t devices 'device' serials
            return
            ;;
{cases}    esac

    _arguments '--json[machine-readable output]' \
               '--quiet[suppress confirmation]' \
               '--dry-run[do not write anything]' \
               '--device[target one device]:serial:'
}}

_dspi "$@"
"#
    )
}

fn fish() -> String {
    let mut out = String::from(
        "# dspi completion for fish. Generated; do not edit.\n\
         # Install:  dspi completions fish > ~/.config/fish/completions/dspi.fish\n\n\
         complete -c dspi -f\n\n",
    );

    for (n, d) in VERBS {
        out.push_str(&format!(
            "complete -c dspi -n '__fish_use_subcommand' -a '{n}' -d '{}'\n",
            d.replace('\'', "")
        ));
    }
    out.push('\n');

    for d in REGISTRY
        .iter()
        .filter(|d| d.set.is_some() || d.get.is_some())
    {
        out.push_str(&format!(
            "complete -c dspi -a '{}' -d '{}'\n",
            d.path,
            d.plain.replace('\'', "")
        ));
    }
    out.push('\n');

    for (path, values) in enum_paths() {
        out.push_str(&format!(
            "complete -c dspi -n '__fish_seen_subcommand_from {path}' -a '{}'\n",
            values.join(" ")
        ));
    }

    out.push_str(
        "\ncomplete -c dspi -l json -d 'machine-readable output'\n\
         complete -c dspi -l quiet -d 'suppress confirmation'\n\
         complete -c dspi -l dry-run -d 'do not write anything'\n\
         complete -c dspi -l device -d 'target one device' -r\n",
    );
    out
}

fn powershell() -> String {
    let words: Vec<String> = verbs()
        .into_iter()
        .chain(paths())
        .map(|s| format!("'{s}'"))
        .collect();

    format!(
        r#"# dspi completion for PowerShell. Generated; do not edit.
# Install:  dspi completions powershell | Out-String | Invoke-Expression
#           (add that line to $PROFILE to make it permanent)

Register-ArgumentCompleter -Native -CommandName dspi -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)

    $words = @({words})

    $words | Where-Object {{ $_ -like "$wordToComplete*" }} | ForEach-Object {{
        [System.Management.Automation.CompletionResult]::new(
            $_, $_, 'ParameterValue', $_)
    }}
}}
"#,
        words = words.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_generates_something() {
        for shell in SHELLS {
            let script = generate(shell).unwrap_or_else(|| panic!("no script for {shell}"));
            assert!(script.len() > 200, "{shell} script looks empty");
            assert!(
                script.contains("vol.user"),
                "{shell} script should carry parameter paths"
            );
        }
    }

    #[test]
    fn an_unknown_shell_is_refused_rather_than_guessed() {
        assert!(generate("tcsh").is_none());
    }

    /// The scripts exist so a user can discover parameters without reading docs;
    /// if they drift from the registry they actively mislead.
    #[test]
    fn scripts_cover_the_whole_registry() {
        let expected = paths().len();
        for shell in ["bash", "zsh", "fish"] {
            let script = generate(shell).unwrap();
            let found = paths().iter().filter(|p| script.contains(**p)).count();
            assert_eq!(found, expected, "{shell} is missing parameters");
        }
    }

    #[test]
    fn enum_values_are_offered_where_they_apply() {
        let bash = generate("bash").unwrap();
        assert!(bash.contains("in.source)"));
        assert!(bash.contains("usb spdif i2s adat"));
    }

    /// A stray quote in a description would break the generated script at source
    /// rather than at review time, so descriptions are sanitised.
    #[test]
    fn descriptions_cannot_break_the_script() {
        for shell in SHELLS {
            let script = generate(shell).unwrap();
            for line in script.lines() {
                // In the quoted-description sections, quotes must be balanced.
                if line.trim_start().starts_with('\'') {
                    assert_eq!(
                        line.matches('\'').count() % 2,
                        0,
                        "{shell}: unbalanced quote in `{line}`"
                    );
                }
            }
        }
    }

    #[test]
    fn bash_and_zsh_complete_device_serials() {
        assert!(generate("bash").unwrap().contains("dspi list"));
        assert!(generate("zsh").unwrap().contains("dspi list"));
    }
}
