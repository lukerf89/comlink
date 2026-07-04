use crate::deps::{self, DependencyReport, DependencyState};

pub fn run() -> bool {
    let report = deps::inspect();
    print_report(&report);

    report.ffmpeg.is_ok() && report.whisper_cpp.is_ok() && report.whisper_model.is_ok()
}

fn print_report(report: &DependencyReport) {
    eprintln!("Comlink doctor");
    eprintln!("  offline core: enabled");
    print_state(
        "ffmpeg",
        &report.ffmpeg,
        "set COMLINK_FFMPEG or install ffmpeg",
    );
    print_state(
        "ffprobe",
        &report.ffprobe,
        "optional but recommended; set COMLINK_FFPROBE or install ffprobe",
    );
    print_state(
        "whisper.cpp",
        &report.whisper_cpp,
        "set COMLINK_WHISPER_CPP to whisper-cli/main or install whisper.cpp",
    );
    print_state(
        "whisper model",
        &report.whisper_model,
        "set COMLINK_WHISPER_MODEL to a ggml model file",
    );
}

fn print_state(label: &str, state: &DependencyState, help: &str) {
    match state {
        DependencyState::Found(path) => eprintln!("  [ok]   {label}: {}", path.display()),
        DependencyState::Missing => eprintln!("  [miss] {label}: {help}"),
        DependencyState::NotExecutable(path) => {
            eprintln!("  [bad]  {label}: {} ({help})", path.display())
        }
    }
}
