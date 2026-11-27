//! The read-only list (t-132606): the programs, verbs and options a command
//! may use and still be proven read-only. Anything not listed — a program, a
//! verb, an option — is not proven, however harmless it looks: a list of what
//! writes is never complete (`python x.py`, `make`, `curl -T`, `git config`,
//! `find -fprint`), a list of what only reads can be checked. Shaped like the
//! inspection shell's table (zo's `bash_validation::inspection`), which reads
//! git's options by the lists kept here.

/// How the words after a listed program (and its verb) are read.
#[derive(Clone, Copy)]
enum Reading {
    /// Getopt options: short ones may be clustered (`-la`), a value-taking
    /// one carries the rest of its cluster as its value (`-d,`, `-n5`), a
    /// bare count (`-20`) is a number, and `--` ends the options.
    Flags,
    /// Every option a whole word of its own: `find`'s `-name`, `-maxdepth`.
    /// A number with a sign (`-mtime -1`, `-size -10k`) is a value.
    Words,
    /// A rule of its own over the words after the program and verb.
    Rule(Rule),
}

#[derive(Clone, Copy)]
enum Rule {
    /// `git status`, `log`, `diff`, `show`: [`git_read_options`].
    GitRead,
    /// `git branch` listing: [`branch_query`].
    GitBranch,
    /// `git remote` reading: [`remote_query`].
    GitRemote,
    /// `sed -n '<N>p'`: [`sed_prints_lines`].
    SedPrint,
    /// `uniq` on its input: [`uniq_reads_its_input`].
    UniqInput,
}

impl Rule {
    fn holds(self, args: &[&str]) -> bool {
        match self {
            Self::GitRead => git_read_options(args),
            Self::GitBranch => branch_query(args),
            Self::GitRemote => remote_query(args),
            Self::SedPrint => sed_prints_lines(args),
            Self::UniqInput => uniq_reads_its_input(args),
        }
    }
}

struct Query {
    program: &'static str,
    subcommand: Option<&'static str>,
    /// Every option it may carry: `--long` (read by its name before `=`),
    /// `-x` (a short one), or `-name` (a whole word, [`Reading::Words`]).
    options: &'static [&'static str],
    /// The short options whose value is the rest of their cluster.
    values: &'static str,
    reading: Reading,
    /// Some option it is not listed with writes a file or runs a program, so
    /// a word the shell may still turn into an option — one that starts with
    /// an unquoted `*`, `?`, `[` or `{` — is refused.
    armed: bool,
}

const fn flags(
    program: &'static str,
    options: &'static [&'static str],
    values: &'static str,
) -> Query {
    Query {
        program,
        subcommand: None,
        options,
        values,
        reading: Reading::Flags,
        armed: false,
    }
}

const fn rule(program: &'static str, subcommand: Option<&'static str>, rule: Rule) -> Query {
    Query {
        program,
        subcommand,
        options: &[],
        values: "",
        reading: Reading::Rule(rule),
        armed: true,
    }
}

impl Query {
    const fn armed(self) -> Self {
        Self {
            armed: true,
            ..self
        }
    }

    const fn verb(self, subcommand: &'static str) -> Self {
        Self {
            subcommand: Some(subcommand),
            ..self
        }
    }
}

const LS: &[&str] = &[
    "-a",
    "-A",
    "-b",
    "-B",
    "-c",
    "-C",
    "-d",
    "-e",
    "-f",
    "-F",
    "-g",
    "-G",
    "-h",
    "-H",
    "-i",
    "-k",
    "-l",
    "-L",
    "-m",
    "-n",
    "-o",
    "-O",
    "-p",
    "-q",
    "-r",
    "-R",
    "-s",
    "-S",
    "-t",
    "-T",
    "-u",
    "-U",
    "-v",
    "-w",
    "-x",
    "-X",
    "-1",
    "-@",
    "--all",
    "--almost-all",
    "--author",
    "--block-size",
    "--classify",
    "--color",
    "--directory",
    "--dereference",
    "--file-type",
    "--format",
    "--full-time",
    "--group-directories-first",
    "--hide",
    "--human-readable",
    "--ignore",
    "--ignore-backups",
    "--indicator-style",
    "--inode",
    "--literal",
    "--no-group",
    "--numeric-uid-gid",
    "--quote-name",
    "--quoting-style",
    "--recursive",
    "--reverse",
    "--si",
    "--size",
    "--sort",
    "--time",
    "--time-style",
    "--width",
];
const CAT: &[&str] = &[
    "-A",
    "-b",
    "-e",
    "-E",
    "-n",
    "-s",
    "-t",
    "-T",
    "-u",
    "-v",
    "--number",
    "--number-nonblank",
    "--show-all",
    "--show-ends",
    "--show-nonprinting",
    "--show-tabs",
    "--squeeze-blank",
];
const HEAD: &[&str] = &[
    "-c",
    "-n",
    "-q",
    "-v",
    "-z",
    "--bytes",
    "--lines",
    "--quiet",
    "--silent",
    "--verbose",
    "--zero-terminated",
];
const TAIL: &[&str] = &[
    "-b",
    "-c",
    "-f",
    "-F",
    "-n",
    "-q",
    "-r",
    "-s",
    "-v",
    "-z",
    "--bytes",
    "--follow",
    "--lines",
    "--pid",
    "--quiet",
    "--retry",
    "--silent",
    "--sleep-interval",
    "--verbose",
    "--zero-terminated",
];
const WC: &[&str] = &[
    "-c",
    "-l",
    "-L",
    "-m",
    "-w",
    "--bytes",
    "--chars",
    "--lines",
    "--max-line-length",
    "--words",
    "--total",
];
const STAT: &[&str] = &[
    "-c",
    "-f",
    "-F",
    "-l",
    "-L",
    "-n",
    "-q",
    "-r",
    "-s",
    "-t",
    "-x",
    "--dereference",
    "--file-system",
    "--format",
    "--printf",
    "--terse",
];
const GREP: &[&str] = &[
    "-a",
    "-A",
    "-b",
    "-B",
    "-c",
    "-C",
    "-d",
    "-D",
    "-e",
    "-E",
    "-f",
    "-F",
    "-G",
    "-h",
    "-H",
    "-i",
    "-I",
    "-l",
    "-L",
    "-m",
    "-n",
    "-o",
    "-P",
    "-q",
    "-r",
    "-R",
    "-s",
    "-T",
    "-U",
    "-v",
    "-w",
    "-x",
    "-z",
    "--after-context",
    "--basic-regexp",
    "--before-context",
    "--binary",
    "--binary-files",
    "--byte-offset",
    "--color",
    "--colour",
    "--context",
    "--count",
    "--dereference-recursive",
    "--devices",
    "--directories",
    "--exclude",
    "--exclude-dir",
    "--exclude-from",
    "--extended-regexp",
    "--file",
    "--files-with-matches",
    "--files-without-match",
    "--fixed-strings",
    "--ignore-case",
    "--include",
    "--initial-tab",
    "--invert-match",
    "--label",
    "--line-buffered",
    "--line-number",
    "--line-regexp",
    "--max-count",
    "--no-filename",
    "--no-ignore-case",
    "--no-messages",
    "--null-data",
    "--only-matching",
    "--perl-regexp",
    "--quiet",
    "--recursive",
    "--regexp",
    "--silent",
    "--text",
    "--with-filename",
    "--word-regexp",
];
// Left out: `--pre` and `--pre-glob` run a program on every file, `-z` and
// `--search-zip` run decompressors, `--hostname-bin` runs one.
const RG: &[&str] = &[
    "-a",
    "-A",
    "-B",
    "-c",
    "-C",
    "-e",
    "-E",
    "-f",
    "-F",
    "-g",
    "-h",
    "-H",
    "-i",
    "-I",
    "-j",
    "-l",
    "-L",
    "-m",
    "-M",
    "-n",
    "-N",
    "-o",
    "-p",
    "-P",
    "-q",
    "-r",
    "-s",
    "-S",
    "-t",
    "-T",
    "-u",
    "-U",
    "-v",
    "-w",
    "-x",
    "-0",
    "-.",
    "--after-context",
    "--before-context",
    "--byte-offset",
    "--case-sensitive",
    "--color",
    "--colors",
    "--column",
    "--context",
    "--count",
    "--count-matches",
    "--crlf",
    "--encoding",
    "--file",
    "--files",
    "--files-with-matches",
    "--files-without-match",
    "--fixed-strings",
    "--follow",
    "--glob",
    "--heading",
    "--hidden",
    "--iglob",
    "--ignore-case",
    "--ignore-file",
    "--invert-match",
    "--json",
    "--line-number",
    "--line-regexp",
    "--max-columns",
    "--max-count",
    "--max-depth",
    "--max-filesize",
    "--multiline",
    "--multiline-dotall",
    "--no-config",
    "--no-filename",
    "--no-heading",
    "--no-ignore",
    "--no-ignore-vcs",
    "--no-line-number",
    "--no-messages",
    "--null",
    "--null-data",
    "--only-matching",
    "--passthru",
    "--pcre2",
    "--pretty",
    "--quiet",
    "--regexp",
    "--replace",
    "--smart-case",
    "--sort",
    "--sortr",
    "--stats",
    "--text",
    "--threads",
    "--trim",
    "--type",
    "--type-list",
    "--type-not",
    "--unrestricted",
    "--vimgrep",
    "--with-filename",
    "--word-regexp",
];
// Left out: `-exec`, `-execdir`, `-ok`, `-okdir` run a program, `-delete`
// deletes, `-fls` and `-fprint*` write a file (the core's
// `FIND_MUTATING_PRIMARIES`).
const FIND: &[&str] = &[
    "-name",
    "-iname",
    "-path",
    "-ipath",
    "-wholename",
    "-iwholename",
    "-regex",
    "-iregex",
    "-regextype",
    "-lname",
    "-ilname",
    "-type",
    "-xtype",
    "-size",
    "-empty",
    "-newer",
    "-anewer",
    "-cnewer",
    "-mtime",
    "-mmin",
    "-atime",
    "-amin",
    "-ctime",
    "-cmin",
    "-perm",
    "-user",
    "-group",
    "-uid",
    "-gid",
    "-nouser",
    "-nogroup",
    "-links",
    "-inum",
    "-samefile",
    "-maxdepth",
    "-mindepth",
    "-depth",
    "-prune",
    "-print",
    "-print0",
    "-printf",
    "-ls",
    "-not",
    "-and",
    "-or",
    "-a",
    "-o",
    "-follow",
    "-xdev",
    "-mount",
    "-noleaf",
    "-readable",
    "-writable",
    "-executable",
    "-true",
    "-false",
    "-quit",
    "-L",
    "-H",
    "-P",
    "-E",
    "-X",
    "-s",
    "-x",
    "-d",
];
const CUT: &[&str] = &[
    "-b",
    "-c",
    "-d",
    "-f",
    "-n",
    "-s",
    "-z",
    "--bytes",
    "--characters",
    "--complement",
    "--delimiter",
    "--fields",
    "--only-delimited",
    "--output-delimiter",
    "--zero-terminated",
];
const TR: &[&str] = &[
    "-c",
    "-C",
    "-d",
    "-s",
    "-u",
    "--complement",
    "--delete",
    "--squeeze-repeats",
    "--truncate-set1",
];
// Left out: `-o`/`--output` writes the result, `--compress-program` runs one,
// `-T` writes its temporary files where it is told.
const SORT: &[&str] = &[
    "-b",
    "-c",
    "-C",
    "-d",
    "-f",
    "-g",
    "-h",
    "-i",
    "-k",
    "-m",
    "-M",
    "-n",
    "-r",
    "-R",
    "-s",
    "-S",
    "-t",
    "-u",
    "-V",
    "-z",
    "--check",
    "--dictionary-order",
    "--field-separator",
    "--general-numeric-sort",
    "--human-numeric-sort",
    "--ignore-case",
    "--ignore-leading-blanks",
    "--ignore-nonprinting",
    "--key",
    "--merge",
    "--month-sort",
    "--numeric-sort",
    "--parallel",
    "--random-sort",
    "--reverse",
    "--stable",
    "--unique",
    "--version-sort",
    "--zero-terminated",
];
const UNIQ: &[&str] = &[
    "-c",
    "-d",
    "-D",
    "-f",
    "-i",
    "-s",
    "-u",
    "-w",
    "-z",
    "--all-repeated",
    "--check-chars",
    "--count",
    "--group",
    "--ignore-case",
    "--repeated",
    "--skip-chars",
    "--skip-fields",
    "--unique",
    "--zero-terminated",
];
const DIFF: &[&str] = &[
    "-a",
    "-b",
    "-B",
    "-c",
    "-C",
    "-d",
    "-e",
    "-E",
    "-i",
    "-I",
    "-N",
    "-p",
    "-q",
    "-r",
    "-s",
    "-t",
    "-T",
    "-u",
    "-U",
    "-w",
    "-W",
    "-x",
    "-X",
    "-y",
    "-Z",
    "--brief",
    "--color",
    "--context",
    "--exclude",
    "--exclude-from",
    "--expand-tabs",
    "--from-file",
    "--ignore-all-space",
    "--ignore-blank-lines",
    "--ignore-case",
    "--ignore-matching-lines",
    "--ignore-space-change",
    "--ignore-tab-expansion",
    "--ignore-trailing-space",
    "--label",
    "--minimal",
    "--new-file",
    "--no-dereference",
    "--normal",
    "--recursive",
    "--report-identical-files",
    "--show-c-function",
    "--side-by-side",
    "--strip-trailing-cr",
    "--suppress-common-lines",
    "--text",
    "--to-file",
    "--unidirectional-new-file",
    "--unified",
    "--width",
];
const CMP: &[&str] = &[
    "-b",
    "-i",
    "-l",
    "-n",
    "-s",
    "--bytes",
    "--ignore-initial",
    "--print-bytes",
    "--quiet",
    "--silent",
    "--verbose",
];
const DU: &[&str] = &[
    "-a",
    "-A",
    "-c",
    "-d",
    "-g",
    "-h",
    "-H",
    "-k",
    "-L",
    "-m",
    "-P",
    "-s",
    "-x",
    "--all",
    "--apparent-size",
    "--block-size",
    "--bytes",
    "--count-links",
    "--dereference",
    "--exclude",
    "--human-readable",
    "--max-depth",
    "--one-file-system",
    "--si",
    "--summarize",
    "--threshold",
    "--total",
];
const DF: &[&str] = &[
    "-a",
    "-g",
    "-h",
    "-H",
    "-i",
    "-k",
    "-l",
    "-m",
    "-P",
    "-T",
    "--all",
    "--block-size",
    "--exclude-type",
    "--human-readable",
    "--inodes",
    "--local",
    "--output",
    "--portability",
    "--print-type",
    "--si",
    "--total",
    "--type",
];
// Left out: `-o` writes the listing to a file, `-H`/`-R` write HTML pages.
const TREE: &[&str] = &[
    "-a",
    "-C",
    "-d",
    "-D",
    "-f",
    "-F",
    "-g",
    "-h",
    "-i",
    "-I",
    "-J",
    "-l",
    "-L",
    "-n",
    "-p",
    "-P",
    "-r",
    "-s",
    "-t",
    "-u",
    "-v",
    "-x",
    "-X",
    "--charset",
    "--dirsfirst",
    "--du",
    "--filelimit",
    "--filesfirst",
    "--gitignore",
    "--matchdirs",
    "--noreport",
    "--prune",
    "--si",
    "--sort",
];
// Left out: `-C` compiles a magic file, `-z`/`-Z` may run decompressors.
const FILE: &[&str] = &[
    "-b",
    "-h",
    "-i",
    "-I",
    "-k",
    "-L",
    "--brief",
    "--dereference",
    "--keep-going",
    "--mime",
    "--mime-encoding",
    "--mime-type",
    "--no-dereference",
];
const JQ: &[&str] = &[
    "-a",
    "-c",
    "-C",
    "-e",
    "-f",
    "-j",
    "-M",
    "-n",
    "-r",
    "-R",
    "-s",
    "-S",
    "--arg",
    "--args",
    "--argjson",
    "--ascii-output",
    "--color-output",
    "--compact-output",
    "--exit-status",
    "--from-file",
    "--indent",
    "--join-output",
    "--jsonargs",
    "--monochrome-output",
    "--null-input",
    "--raw-input",
    "--raw-output",
    "--raw-output0",
    "--rawfile",
    "--seq",
    "--slurp",
    "--slurpfile",
    "--sort-keys",
    "--stream",
    "--tab",
];
const BASENAME: &[&str] = &["-a", "-s", "-z", "--multiple", "--suffix", "--zero"];
const REALPATH: &[&str] = &[
    "-e",
    "-L",
    "-m",
    "-P",
    "-q",
    "-s",
    "-z",
    "--canonicalize-existing",
    "--canonicalize-missing",
    "--logical",
    "--no-symlinks",
    "--physical",
    "--quiet",
    "--relative-base",
    "--relative-to",
    "--strip",
    "--zero",
];
const READLINK: &[&str] = &[
    "-e",
    "-f",
    "-m",
    "-n",
    "-q",
    "-s",
    "-v",
    "-z",
    "--canonicalize",
    "--canonicalize-existing",
    "--canonicalize-missing",
    "--no-newline",
    "--quiet",
    "--silent",
    "--verbose",
    "--zero",
];
const UNAME: &[&str] = &[
    "-a",
    "-i",
    "-m",
    "-n",
    "-o",
    "-p",
    "-r",
    "-s",
    "-v",
    "--all",
    "--hardware-platform",
    "--kernel-name",
    "--kernel-release",
    "--kernel-version",
    "--machine",
    "--nodename",
    "--operating-system",
    "--processor",
];
const SEQ: &[&str] = &[
    "-f",
    "-s",
    "-t",
    "-w",
    "--equal-width",
    "--format",
    "--separator",
];
const PASTE: &[&str] = &[
    "-d",
    "-s",
    "-z",
    "--delimiters",
    "--serial",
    "--zero-terminated",
];
const GIT_REV_PARSE: &[&str] = &[
    "-q",
    "--abbrev-ref",
    "--absolute-git-dir",
    "--git-common-dir",
    "--git-dir",
    "--is-inside-work-tree",
    "--quiet",
    "--short",
    "--show-cdup",
    "--show-prefix",
    "--show-toplevel",
    "--symbolic-full-name",
    "--verify",
];
const GIT_LS_FILES: &[&str] = &[
    "-c",
    "-d",
    "-i",
    "-m",
    "-o",
    "-s",
    "-t",
    "-z",
    "--cached",
    "--deleted",
    "--directory",
    "--error-unmatch",
    "--exclude-standard",
    "--full-name",
    "--ignored",
    "--modified",
    "--others",
    "--stage",
];
/// zo's ledger reporting verbs (probe B1): a read-only worker reports only
/// through them, so they stay provable. They change the ledger, never a file.
const ORC: &[&str] = &[
    "--ack",
    "--all",
    "--body",
    "--body-file",
    "--dispatch",
    "--format",
    "--payload",
    "--peek",
    "--resume",
    "--retry-request",
    "--run",
    "--task",
    "--timeout-ms",
    "--to",
    "--to-message",
    "--type",
    "--types",
    "--wait",
];

const QUERIES: &[Query] = &[
    flags("ls", LS, ""),
    flags("cat", CAT, ""),
    flags("head", HEAD, "cn"),
    flags("tail", TAIL, "bcns"),
    flags("wc", WC, ""),
    flags("stat", STAT, "cft"),
    flags("grep", GREP, "ABCdDefm"),
    flags("egrep", GREP, "ABCdDefm"),
    flags("fgrep", GREP, "ABCdDefm"),
    flags("rg", RG, "ABCeEfgjmMtT").armed(),
    Query {
        reading: Reading::Words,
        ..flags("find", FIND, "").armed()
    },
    rule("sed", None, Rule::SedPrint),
    flags("echo", &["-n", "-e", "-E"], ""),
    flags("printf", &[], ""),
    flags("pwd", &["-L", "-P"], ""),
    flags("cd", &["-L", "-P"], ""),
    flags("which", &["-a", "-s"], ""),
    flags("true", &[], ""),
    flags("false", &[], ""),
    flags("basename", BASENAME, "s"),
    flags("dirname", &["-z", "--zero"], ""),
    flags("realpath", REALPATH, ""),
    flags("readlink", READLINK, ""),
    flags("cut", CUT, "bcdf"),
    flags("tr", TR, ""),
    flags("sort", SORT, "kStT").armed(),
    Query {
        reading: Reading::Rule(Rule::UniqInput),
        ..flags("uniq", UNIQ, "fsw")
    },
    flags("paste", PASTE, "d"),
    flags("seq", SEQ, "fst"),
    flags("diff", DIFF, "CUWxXI"),
    flags("cmp", CMP, "in"),
    flags("du", DU, "dB"),
    flags("df", DF, "t"),
    flags("tree", TREE, "ILP").armed(),
    flags("file", FILE, "").armed(),
    flags("jq", JQ, "f"),
    flags("uname", UNAME, ""),
    flags("whoami", &[], ""),
    rule("git", Some("status"), Rule::GitRead),
    rule("git", Some("log"), Rule::GitRead),
    rule("git", Some("diff"), Rule::GitRead),
    rule("git", Some("show"), Rule::GitRead),
    rule("git", Some("branch"), Rule::GitBranch),
    rule("git", Some("remote"), Rule::GitRemote),
    flags("git", GIT_REV_PARSE, "").verb("rev-parse").armed(),
    flags("git", GIT_LS_FILES, "").verb("ls-files").armed(),
    flags("zerocode-orc", ORC, "").verb("send"),
    flags("zerocode-orc", ORC, "").verb("ask"),
    flags("zerocode-orc", ORC, "").verb("check"),
    flags("zerocode-orc", ORC, "").verb("reply"),
    flags("zerocode-orc", ORC, "").verb("run-current"),
];

const BRANCH_OPTIONS: &[&str] = &[
    "--list",
    "-l",
    "-a",
    "--all",
    "-r",
    "--remotes",
    "-v",
    "-vv",
    "--verbose",
    "--show-current",
    "--no-color",
    "--contains",
    "--no-contains",
    "--merged",
    "--no-merged",
    "--format",
    "--sort",
];
// Positive option spellings are deliberate: Git accepts abbreviations such
// as --out for --output, so a blacklist of write flags would not be a guard.
const GIT_READ_OPTIONS: &[&str] = &[
    "--short",
    "--branch",
    "--porcelain",
    "--ignored",
    "--untracked-files",
    "--oneline",
    "--format",
    "--pretty",
    "--max-count",
    "--since",
    "--until",
    "--after",
    "--before",
    "--author",
    "--committer",
    "--grep",
    "--all",
    "--branches",
    "--tags",
    "--remotes",
    "--graph",
    "--decorate",
    "--date",
    "--no-walk",
    "--reverse",
    "--first-parent",
    "--merges",
    "--no-merges",
    "--name-only",
    "--name-status",
    "--stat",
    "--numstat",
    "--shortstat",
    "--summary",
    "--raw",
    "--patch",
    "--no-patch",
    "--no-color",
    "--color",
    "--check",
    "--exit-code",
    "--quiet",
    "--cached",
    "--staged",
    "--no-index",
    "--diff-filter",
    "--unified",
    "--ignore-all-space",
    "--ignore-space-change",
];
const GIT_READ_SHORT_OPTIONS: &[&str] = &[
    "-s", "-b", "-sb", "-bs", "-z", "-p", "-u", "-n", "-w", "-M", "-C", "-R", "-r", "-m", "-c",
];

/// Whether every option of a `git status`, `log`, `diff` or `show` is one
/// that only reads; the words after `--` are paths.
pub fn git_read_options<S: AsRef<str>>(args: &[S]) -> bool {
    let mut paths = false;
    args.iter().map(AsRef::as_ref).all(|arg| {
        if arg == "--" {
            paths = true;
        }
        if paths || !arg.starts_with('-') {
            return true;
        }
        GIT_READ_OPTIONS.contains(&arg.split('=').next().unwrap_or_default())
            || GIT_READ_SHORT_OPTIONS.contains(&arg)
            || arg.strip_prefix('-').is_some_and(|count| {
                !count.is_empty() && count.chars().all(|ch| ch.is_ascii_digit())
            })
            || ["-n", "-U"].iter().any(|prefix| {
                arg.strip_prefix(prefix).is_some_and(|count| {
                    !count.is_empty() && count.chars().all(|ch| ch.is_ascii_digit())
                })
            })
    })
}

/// Whether a `git branch` only lists: every word one of its listing options
/// (a bare name would make a branch).
pub fn branch_query<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter()
        .all(|arg| BRANCH_OPTIONS.contains(&arg.as_ref().split('=').next().unwrap_or_default()))
}

/// Whether a `git remote` only reads: bare, `-v`, or `get-url <name>`.
pub fn remote_query<S: AsRef<str>>(args: &[S]) -> bool {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    match args.as_slice() {
        [] => true,
        [flag] if matches!(*flag, "-v" | "--verbose") => true,
        ["get-url", rest @ ..] => {
            rest.iter().filter(|arg| !arg.starts_with('-')).count() == 1
                && rest
                    .iter()
                    .all(|arg| !arg.starts_with('-') || matches!(*arg, "--all" | "--push"))
        }
        _ => false,
    }
}

/// `sed -n '<N>p'` or `sed -n '<N>,<M>p'` over files: it prints lines and
/// nothing else (openai/codex's one `sed` form). Any other script may write
/// (`w`, `-i`) or run (`e`) something.
fn sed_prints_lines(args: &[&str]) -> bool {
    let address =
        |part: &str| part == "$" || (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    match args {
        ["-n", script, files @ ..] => {
            script
                .strip_suffix('p')
                .is_some_and(|range| match range.split_once(',') {
                    Some((from, to)) => address(from) && address(to),
                    None => address(range),
                })
                && files.iter().all(|file| !file.starts_with('-'))
        }
        _ => false,
    }
}

/// `uniq` with listed options reading its input: a second file name would be
/// the file it writes.
fn uniq_reads_its_input(args: &[&str]) -> bool {
    args.iter()
        .all(|arg| arg.starts_with('-') && arg.len() > 1 && flag_listed(arg, UNIQ, "fsw"))
}

/// Why a simple command — its words as the shell hands them over, none of
/// them filled in at run time — is not proven read-only, or `None` when the
/// list holds its program, verb and every option.
pub(super) fn refusal(words: &[(&str, bool)]) -> Option<String> {
    let Some((&(program, _), rest)) = words.split_first() else {
        return Some("A line that runs no program is not on the read-only list, so it is not allowed in read-only mode".to_string());
    };
    let texts: Vec<&str> = rest.iter().map(|(text, _)| *text).collect();
    let mut args = texts.as_slice();
    if program == "git" {
        args = match git_globals(args) {
            Some(after) => after,
            None => {
                return Some(format!(
                    "A git option before the verb in '{}' is not on the read-only list, so it is not allowed in read-only mode",
                    words_of(words)
                ));
            }
        };
    }
    let rows: Vec<&Query> = QUERIES
        .iter()
        .filter(|query| query.program == program)
        .collect();
    let query = match rows.as_slice() {
        [] => {
            return Some(format!(
                "Command '{program}' is not on the read-only list, so it is not allowed in read-only mode"
            ));
        }
        [only] if only.subcommand.is_none() => *only,
        _ => {
            let verb = args.first().copied().unwrap_or_default();
            let Some(query) = rows.iter().find(|query| query.subcommand == Some(verb)) else {
                return Some(format!(
                    "'{program} {verb}' is not on the read-only list, so it is not allowed in read-only mode"
                ));
            };
            args = &args[1..];
            *query
        }
    };
    let at = words.len() - args.len();
    if query.armed
        && let Some((word, _)) = words[at..]
            .iter()
            .take_while(|(text, _)| matches!(query.reading, Reading::Words) || *text != "--")
            .find(|(_, glob)| *glob)
    {
        return Some(format!(
            "'{word}' is a pattern the shell may turn into an option of '{program}', so it is not allowed in read-only mode"
        ));
    }
    match query.reading {
        Reading::Flags => args
            .iter()
            .take_while(|arg| **arg != "--")
            .find(|arg| arg.starts_with('-') && **arg != "-" && !flag_listed(arg, query.options, query.values))
            .map(|arg| format!("Option '{arg}' of '{program}' is not on the read-only list, so it is not allowed in read-only mode")),
        Reading::Words => args
            .iter()
            .find(|arg| arg.starts_with('-') && !query.options.contains(arg) && !signed_number(arg))
            .map(|arg| format!("'{program} {arg}' is not on the read-only list, so it is not allowed in read-only mode")),
        Reading::Rule(rule) => (!rule.holds(args)).then(|| {
            format!("'{}' is not in a form on the read-only list, so it is not allowed in read-only mode", words_of(words))
        }),
    }
}

/// git's options before its verb: `-C <folder>` and `--no-pager` only —
/// `-c` sets configuration (an alias, a pager, an external diff) and
/// `--git-dir`/`--work-tree` move the repository.
fn git_globals<'a>(mut args: &'a [&'a str]) -> Option<&'a [&'a str]> {
    loop {
        match args {
            ["-C", _, rest @ ..] => args = rest,
            ["--no-pager", rest @ ..] => args = rest,
            [first, ..] if first.starts_with('-') => return None,
            _ => return Some(args),
        }
    }
}

/// Whether one getopt word is listed: a `--long` by its name, a count
/// (`-20`), or a cluster of listed short options whose value-taking one
/// carries the rest (`-la`, `-n5`, `-d,`).
fn flag_listed(arg: &str, options: &[&str], values: &str) -> bool {
    if arg.starts_with("--") {
        return options.contains(&arg.split('=').next().unwrap_or(arg));
    }
    let letters = &arg[1..];
    if !letters.is_empty() && letters.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    for letter in letters.chars() {
        let listed = options.iter().any(|option| {
            option
                .strip_prefix('-')
                .is_some_and(|short| short.chars().eq([letter]))
        });
        if !listed {
            return false;
        }
        if values.contains(letter) {
            return true;
        }
    }
    true
}

/// `-1`, `-10k`, `-644`: a value with a sign (find's `-mtime -1`), never a primary.
fn signed_number(arg: &str) -> bool {
    let digits = arg[1..].trim_end_matches(|c: char| c.is_ascii_alphabetic());
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn words_of(words: &[(&str, bool)]) -> String {
    words
        .iter()
        .map(|(text, _)| *text)
        .collect::<Vec<_>>()
        .join(" ")
}
