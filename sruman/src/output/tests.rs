use super::*;

fn output() -> Output {
    Output {
        color: ColorChoice::Never,
        json: false,
        quiet: false,
        yes: false,
    }
}

#[test]
fn tables_align_and_ignore_escapes() {
    let rows = vec![
        vec![
            "sniper".into(),
            "\x1b[1m3.0\x1b[0m".into(),
            "[current]".into(),
        ],
        vec!["scout".into(), "1.0.12".into(), String::new()],
    ];
    assert_eq!(
        table(&rows),
        ["sniper  \x1b[1m3.0\x1b[0m     [current]", "scout   1.0.12"]
    );
    assert!(table(&[]).is_empty());
}

#[test]
fn sizes() {
    assert_eq!(size(0), "0 B");
    assert_eq!(size(1023), "1023 B");
    assert_eq!(size(1024), "1.00 KiB");
    assert_eq!(size(194_772_582), "185.75 MiB");
    assert_eq!(size(3 << 40), "3.00 TiB");
}

#[test]
fn confirm_answers() {
    let ask = |input: &str, interactive: bool, yes: bool, default: bool| {
        let (mut printer, out, _) =
            Printer::capture(&Output { yes, ..output() }, input, interactive);
        let answer = printer.confirm("Proceed?", default).unwrap();
        (answer, out.text())
    };
    assert_eq!(
        ask("y\n", true, false, false),
        (true, ":: Proceed? [y/N] ".into())
    );
    assert!(!ask("no\n", true, false, true).0);
    assert!(ask("\n", true, false, true).0);
    assert!(!ask("", true, false, false).0);
    // `--yes` and no terminal take the default without reading.
    assert_eq!(
        ask("n\n", true, true, true),
        (true, ":: Proceed? [Y/n] \n".into())
    );
    assert!(!ask("y\n", false, false, false).0);
}

#[test]
fn streams_quiet_and_colors() {
    let (mut printer, out, err) = Printer::capture(&output(), "", false);
    printer.heading("Step").unwrap();
    printer.info("info").unwrap();
    printer.line("result").unwrap();
    printer.warning("careful").unwrap();
    printer.error("broken").unwrap();
    printer.note("started").unwrap();
    assert_eq!(out.text(), ":: Step\ninfo\nresult\n");
    assert_eq!(err.text(), "warning: careful\nerror: broken\nstarted\n");

    let (mut printer, out, err) = Printer::capture(
        &Output {
            quiet: true,
            ..output()
        },
        "",
        false,
    );
    printer.heading("Step").unwrap();
    printer.info("info").unwrap();
    printer.note("started").unwrap();
    printer.line("result").unwrap();
    printer.error("broken").unwrap();
    assert_eq!(
        (out.text(), err.text()),
        ("result\n".into(), "error: broken\n".into())
    );

    let (mut printer, _, err) = Printer::capture(
        &Output {
            color: ColorChoice::Always,
            ..output()
        },
        "",
        false,
    );
    printer.error("x").unwrap();
    assert_eq!(err.text(), "\x1b[1;31merror:\x1b[0m x\n");
    assert_eq!(printer.green("ok"), "\x1b[1;32mok\x1b[0m");
}

#[test]
fn progress_without_a_terminal_prints_phases() {
    let (mut printer, _, err) = Printer::capture(&output(), "", false);
    let mut bar = printer.progress();
    bar.begin("sniper 3.0");
    bar.phase(Phase::Download, Some(10));
    bar.advance(10);
    bar.phase(Phase::Extract, None);
    bar.end(true);
    assert_eq!(
        err.text(),
        "downloading sniper 3.0\nextracting sniper 3.0\n"
    );
}
