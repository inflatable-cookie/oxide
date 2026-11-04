#!/usr/bin/env python3
"""The covering Rust unit suite at 600 s with the AGENTS dual-root recipe
(oxide#224).

`effigy test rust` runs the same suite under the shared proof supervisor with
that selector's default 300 s limit, which a loaded host can exhaust (the suite
measured 279.7 s quiet and hit the limit at 300 s under load). This wrapper adds
no supervisor and no monitor: it only packages the AGENTS.md recipe as one
named selector.

    python3 scripts/rust-test-600.py    # effigy rust:test-600

The capture-only `--capture-cargo` choice selects the same fixed test arguments
with Cargo; the default remains Oxide. No other command can be supplied.

Fixed bounds, never overridable from the command line: the shared supervisor
(`scripts/bounded-test.py`, the qualified source bytes) gets `--limit 600
--scratch-budget-mib 512` and the whole command `oxide test --workspace
--all-targets --locked`. Its own deadline, host-reserve watchdog, closure
census and cleanup semantics are unchanged; nothing here supervises, signals or
times the suite.

Two roots, as AGENTS.md specifies for this suite and only this suite:

* the internal root, created with `mkdtemp` under `/private/tmp` and exported as
  both `OXIDE_TASK_SCRATCH_ROOT` and `OXIDE_TEST_INTERNAL_ROOT`, because a few
  macOS fixtures take their premise from the internal boot Data volume and must
  stay inside the monitored root;
* the validation temp root, created with `mkdtemp` under the selected volume's
  private `<mount>/scratch/oxide-validation` parent (the documented sibling of
  the supervisor's marked scratch subtree, so it is outside the seven-day
  self-expiry), exported as `TMPDIR`, `TMP` and `TEMP`. Ordinary test temp
  still lands on the pool's volume.

Setup refuses an inherited control root (`OXIDE_TASK_SCRATCH_ROOT`,
`OXIDE_TEST_INTERNAL_ROOT`, `OXIDE_SCRATCH_ROOT`, `OXIDE_PROOF_EVIDENCE`) rather
than adopting it, refuses a home, mount-root or group/other-writable ancestor,
and validates every root it creates as real, canonical, this user's and exactly
0700 on the expected device. It creates exactly two per-run roots with
`mkdtemp`; the reusable `<mount>/scratch/oxide-validation` parent is created
once if absent and kept, like the supervisor's own scratch leaf.

Cleanup removes exactly those two per-run roots, and only after the supervisor
exits 0 (a closed run). Any other exit — a fault, a deadline, an interrupt — or
any exception keeps both roots in place with their supervisor evidence, and
prints their paths, so an unclosed run is never destroyed. The child
environment is a copy; this process's own environment is never changed.

`--self-check` runs the pure and filesystem-local apparatus checks with no
child, Cargo, pool or suite. `--plan` prints the fixed command, roots and
ownership rules with no effect.
"""
from __future__ import annotations

import importlib.util
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
LIMIT_S = 600
SCRATCH_BUDGET_MIB = 512
COMMAND = ("oxide", "test", "--workspace", "--all-targets", "--locked")
CAPTURE_COMMAND = ("cargo", "test", "--workspace", "--all-targets", "--locked")
INTERNAL_PARENT = "/private/tmp"
VALIDATION_PREFIX = "run."
INTERNAL_PREFIX = "oxide-internal."
# An inherited value is an adoption, not a root this run positively owns: it
# could be shared, a caller's home, or another run's retained evidence.
INHERITED = (
    "OXIDE_TASK_SCRATCH_ROOT",
    "OXIDE_TEST_INTERNAL_ROOT",
    "OXIDE_SCRATCH_ROOT",
    "OXIDE_PROOF_EVIDENCE",
)
ENVIRONMENT_KEYS = ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_TEST_INTERNAL_ROOT",
                    "TMPDIR", "TMP", "TEMP")


def load_bounded():
    """The qualified supervisor's own module, for its selection, preparation
    and leaf-validation seams. Importing it creates nothing."""
    spec = importlib.util.spec_from_file_location(
        "bounded_test_recipe", os.path.join(HERE, "bounded-test.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BOUNDED = load_bounded()


class Refused(Exception):
    """A setup or argument refusal, printed without a traceback."""


def parse(argv):
    options = {"self_check": False, "plan": False, "capture_cargo": False}
    for argument in argv:
        if argument == "--self-check":
            options["self_check"] = True
        elif argument == "--plan":
            options["plan"] = True
        elif argument == "--capture-cargo":
            options["capture_cargo"] = True
        else:
            raise Refused(f"unknown argument {argument}; this selector's bounds are fixed")
    if options["self_check"] and options["plan"]:
        raise Refused("--self-check and --plan are separate modes")
    return options


def refuse_inherited(env):
    """Refuse to adopt a caller's root: this selector owns exactly the roots it
    creates in this run."""
    for name in INHERITED:
        value = env.get(name)
        if isinstance(value, str) and value:
            raise Refused(f"{name} is inherited ({value}); unset it so this run "
                          "creates and owns its roots")


def supervisor_argv(capture_cargo=False):
    """The whole command the shared supervisor runs, with this selector's fixed
    bounds. Nothing here overrides them."""
    return [sys.executable, os.path.join(HERE, "bounded-test.py"),
            "--limit", str(LIMIT_S),
            "--scratch-budget-mib", str(SCRATCH_BUDGET_MIB),
            "--", *(CAPTURE_COMMAND if capture_cargo else COMMAND)]


def supervisor_environment(env, roots):
    """A copy of `env` with the dual-root recipe exported. `env` is never
    mutated."""
    child = dict(env)
    internal = roots["internal"]
    child["OXIDE_TASK_SCRATCH_ROOT"] = internal
    child["OXIDE_TEST_INTERNAL_ROOT"] = internal
    for name in ("TMPDIR", "TMP", "TEMP"):
        child[name] = roots["validation"]
    return child


def refuse_home(path, home):
    """A caller's account home is never a scratch root or below one: a home's
    own 0700 mode would otherwise pass the privacy checks."""
    if not isinstance(home, str) or not home:
        return
    home = home.rstrip(os.sep) or os.sep
    if home == os.sep:  # a root account's home contains every path
        return
    if path == home or path.startswith(home + os.sep):
        raise Refused(f"{path} is inside the account home {home}")


def prepare_validation_parent(selection, probe, make_dir=None):
    """The documented `<mount>/scratch/oxide-validation` parent (AGENTS.md).

    The shared `<mount>/scratch` ancestor must already exist and must not be
    group/other writable; it is never created, chmodded, adopted or removed.
    Only the private leaf is created, with exactly 0700, and it is re-validated
    after creation. This reuses the qualified supervisor's own validators and
    creates nothing else."""
    make_dir = make_dir or os.mkdir
    mount, device = selection.get("mount"), selection.get("device")
    if selection.get("source") != "selected-volume" or not isinstance(mount, str):
        raise Refused("no selected volume to prepare")
    ancestor = os.path.join(mount, BOUNDED.SCRATCH_SUBDIR)
    try:
        BOUNDED.checked_ancestor(ancestor, device, probe)
    except FileNotFoundError:
        raise Refused(f"shared scratch ancestor {ancestor} is absent; Oxide never creates it")
    except OSError as error:
        raise Refused(f"shared scratch ancestor {ancestor} is unreadable: {BOUNDED.type_name(error)}")
    except BOUNDED.SelectionRefused as error:
        raise Refused(str(error))
    parent = BOUNDED.validation_parent(selection)
    try:
        # An existing parent is the documented reusable `mkdir -p` directory, so
        # it is judged like the shared ancestor: real, canonical, this user's and
        # never group/other writable. Only this run's `run.*` root below it must
        # be exactly 0700, and that is checked after creation.
        BOUNDED.checked_ancestor(parent, device, probe)
        return parent
    except FileNotFoundError:
        pass
    except OSError as error:
        raise Refused(f"validation parent {parent} is unreadable: {BOUNDED.type_name(error)}")
    except BOUNDED.SelectionRefused as error:
        raise Refused(str(error))
    try:
        make_dir(parent, 0o700)
    except FileExistsError:
        pass
    except OSError as error:
        raise Refused(f"validation parent {parent} cannot be created: {BOUNDED.type_name(error)}")
    try:
        created = BOUNDED.checked_leaf(parent, device, probe)
        again = probe.lstat(parent)
    except OSError as error:
        raise Refused(f"validation parent {parent} is unreadable: {BOUNDED.type_name(error)}")
    except BOUNDED.SelectionRefused as error:
        raise Refused(str(error))
    if (again.st_dev, again.st_ino) != (created.st_dev, created.st_ino):
        raise Refused(f"validation parent {parent} changed while it was being prepared")
    return parent


def layout(env, selection, probe=None, internal_parent=INTERNAL_PARENT,
           boot_device=None, mkdtemp=None, make_dir=None):
    """Create and validate exactly this run's two roots.

    `selection` is the effect-free pool selection (`select_default_parent`).
    Each root is re-validated after creation; a refusal removes only what this
    call just created and raises."""
    probe = probe if probe is not None else BOUNDED.DefaultProbe()
    mkdtemp = mkdtemp if mkdtemp is not None else tempfile.mkdtemp
    refuse_inherited(env)
    boot_device = boot_device if boot_device is not None else probe.stat("/").st_dev
    try:
        home = probe.home()
    except BOUNDED.SelectionRefused:
        home = None
    facts = {"selection": selection.get("source"), "warning": None,
             "validation_parent": None, "validation": None, "internal": None}
    mount = selection.get("mount") if selection.get("source") == "selected-volume" else None
    refuse_home(internal_parent, home)
    if isinstance(mount, str):
        parent = prepare_validation_parent(selection, probe, make_dir)
        device = selection["device"]
        facts["validation_parent"] = parent
    else:
        parent = internal_parent
        if not isinstance(selection.get("reason"), str):
            raise Refused("no selected volume and no bounded reason for the fallback")
        device = boot_device
        facts["validation_parent"] = parent
        facts["warning"] = (f"no selected volume ({selection['reason']}); validation temp falls back "
                            f"to {parent}, which is not selected-volume coverage")
    try:
        info = probe.lstat(internal_parent)
    except OSError as error:
        raise Refused(f"internal temp parent {internal_parent} is unreadable: {BOUNDED.type_name(error)}")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise Refused(f"internal temp parent {internal_parent} is not a real directory")
    if probe.realpath(internal_parent) != internal_parent:
        raise Refused(f"internal temp parent {internal_parent} is not canonical")
    if probe.stat(internal_parent).st_dev != boot_device:
        raise Refused(f"internal temp parent {internal_parent} is not on the boot volume")
    if probe.realpath(parent) != parent:
        raise Refused(f"validation temp parent {parent} is not canonical")
    refuse_home(parent, home)
    created = []
    try:
        validation = mkdtemp(prefix=VALIDATION_PREFIX, dir=parent)
        created.append(validation)
        BOUNDED.checked_leaf(validation, device, probe)
        internal = mkdtemp(prefix=INTERNAL_PREFIX, dir=internal_parent)
        created.append(internal)
        BOUNDED.checked_leaf(internal, boot_device, probe)
    except (OSError, BOUNDED.SelectionRefused) as error:
        for path in reversed(created):
            shutil.rmtree(path, ignore_errors=True)
        if isinstance(error, BOUNDED.SelectionRefused):
            raise Refused(str(error))
        raise Refused(f"cannot create this run's roots: {BOUNDED.type_name(error)}")
    if (validation == internal or validation.startswith(internal + os.sep)
            or internal.startswith(validation + os.sep)):
        for path in created:
            shutil.rmtree(path, ignore_errors=True)
        raise Refused("the internal and validation roots are not distinct")
    facts.update(validation=validation, internal=internal, device=device,
                 boot_device=boot_device)
    return facts


def distinct_devices(validation_device, boot_device):
    """A selected volume's validation temp must not be the boot volume's; the
    internal root must be. The pool selection already refuses a boot-volume
    mount, so this is the wrapper's own belt-and-braces rule."""
    if validation_device == boot_device:
        raise Refused("the validation temp root is on the boot volume, not the selected one")


def cleanup(roots, code):
    """Remove exactly the two per-run roots, and only for a closed run (exit
    0). Any other outcome keeps them with their evidence and reports them, and
    so does a closed run whose removal did not finish."""
    if code != 0:
        return "kept"
    for name in ("validation", "internal"):
        shutil.rmtree(roots[name], ignore_errors=True)
    if any(os.path.lexists(roots[name]) for name in ("validation", "internal")):
        return "kept"
    return "removed"


def plan_lines(capture_cargo=False):
    return [
        f"rust:test-600 plan (oxide#224): {LIMIT_S} s limit, {SCRATCH_BUDGET_MIB} MiB monitored scratch",
        "command: " + " ".join(supervisor_argv(capture_cargo)),
        f"internal root: one {INTERNAL_PREFIX}* owner-only directory under {INTERNAL_PARENT},",
        "  exported as OXIDE_TASK_SCRATCH_ROOT and OXIDE_TEST_INTERNAL_ROOT",
        "validation temp: one run.* owner-only directory under the selected volume's",
        "  <mount>/scratch/oxide-validation parent, exported as TMPDIR, TMP and TEMP",
        "refused: inherited OXIDE_TASK_SCRATCH_ROOT/OXIDE_TEST_INTERNAL_ROOT/OXIDE_SCRATCH_ROOT/",
        "  OXIDE_PROOF_EVIDENCE, and any home, mount-root or group/other-writable ancestor",
        "cleanup: both per-run roots are removed only after a closed (exit 0) run;",
        "  any other outcome keeps them and prints their paths",
        "no effect in plan mode",
    ]


def run(env, probe=None, runner=None, capture_cargo=False):
    """Prepare this run's roots, run the shared supervisor once, then dispose of
    the roots truthfully. Returns the supervisor's exit status."""
    probe = probe if probe is not None else BOUNDED.DefaultProbe()
    runner = runner if runner is not None else launch
    selection = BOUNDED.select_default_parent(env, probe)
    roots = layout(env, selection, probe)
    if roots["selection"] == "selected-volume":
        distinct_devices(roots["device"], roots["boot_device"])
    if roots["warning"]:
        print(f"rust-test-600: warning: {roots['warning']}", file=sys.stderr)
    print(f"rust-test-600: limit {LIMIT_S} s, monitored scratch {SCRATCH_BUDGET_MIB} MiB; "
          f"internal root {roots['internal']}, validation temp {roots['validation']}",
          flush=True)
    child = supervisor_environment(env, roots)
    code = 1
    try:
        code = runner(supervisor_argv(capture_cargo), child)
    finally:
        disposition = cleanup(roots, code)
        if disposition == "kept":
            print(f"rust-test-600: exit {code}: roots kept for the unclosed run — "
                  f"internal {roots['internal']} (with the supervisor's evidence), "
                  f"validation {roots['validation']}", flush=True)
    return code


def launch(argv, env):
    """One bounded child, with this process's interrupt forwarded so the
    supervisor, not this wrapper, owns the suite's custody. Only a positive
    child exit is returned."""
    child = subprocess.Popen(argv, env=env, cwd=REPO)

    def forward(signum, _frame):
        try:
            child.send_signal(signum)
        except OSError:
            pass

    previous = {}
    for signum in (signal.SIGINT, signal.SIGTERM):
        previous[signum] = signal.signal(signum, forward)
    try:
        return child.wait()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def self_check():
    """No child, no Cargo, no suite: the refusal, environment, command, cleanup
    and ownership rules, on one private temp root this check removes."""
    checks = []

    def check(name, condition):
        checks.append((name, bool(condition)))
        if not condition:
            print(f"rust:test-600 self-check FAILED: {name}", file=sys.stderr)

    def refused(function, *args, **kwargs):
        try:
            function(*args, **kwargs)
        except Refused:
            return True
        return False

    check("parse refuses an unknown argument",
          refused(parse, ["--limit", "900"]))
    check("parse accepts the two effect-free modes",
          parse(["--self-check"])["self_check"] and parse(["--plan"])["plan"])
    check("parse refuses both modes together", refused(parse, ["--self-check", "--plan"]))
    check("capture mode requires its exact flag", parse(["--capture-cargo"])["capture_cargo"]
          and not parse([])["capture_cargo"]
          and refused(parse, ["--capture-cargo", "--arbitrary"]))

    for name in INHERITED:
        check(f"an inherited {name} refuses", refused(refuse_inherited, {name: "/somewhere"}))
    check("an unrelated inherited value is not a refusal",
          not refused(refuse_inherited, {"HOME": "/home/person", "TMPDIR": "/tmp/elsewhere"}))
    check("an account home root refuses",
          refused(refuse_home, "/Users/person", "/Users/person"))
    check("a root inside the account home refuses",
          refused(refuse_home, "/Users/person/scratch/run.1", "/Users/person"))
    check("a sibling of the account home is not a refusal",
          not refused(refuse_home, "/Users/person2/scratch", "/Users/person"))

    argv = supervisor_argv()
    check("the command is the suite with the fixed 600 s and 512 MiB bounds",
          argv[argv.index("--limit") + 1] == "600"
          and argv[argv.index("--scratch-budget-mib") + 1] == "512"
          and tuple(argv[argv.index("--") + 1:]) == COMMAND)
    capture_argv = supervisor_argv(parse(["--capture-cargo"])["capture_cargo"])
    check("capture choice changes only the fixed suite command",
          capture_argv[:-len(CAPTURE_COMMAND)] == argv[:-len(COMMAND)]
          and tuple(capture_argv[capture_argv.index("--") + 1:])
          == ("cargo", "test", "--workspace", "--all-targets", "--locked")
          and COMMAND == ("oxide", "test", "--workspace", "--all-targets", "--locked"))

    # Check the actual Effigy declarations without executing Cargo or a child.
    import tomllib
    with open(os.path.join(REPO, "effigy.toml"), "rb") as config:
        tasks = tomllib.load(config)["tasks"]
    expected_selectors = {
        "rust:compile:cargo": "cargo check --workspace --all-targets --locked",
        "rust:compile-release:cargo": "RUSTFLAGS='-D warnings' cargo check --release --workspace --bins --locked",
        "rust:format:cargo": "cargo fmt --all -- --check",
        "rust:test-600:cargo": "python3 scripts/rust-test-600.py --capture-cargo",
    }
    check("capture selectors bind fixed commands under heavy admission",
          all(tasks.get(name) == {"admission": "heavy", "run": command}
              for name, command in expected_selectors.items()))
    check("ordinary Rust selectors retain the Oxide backend",
          tasks.get("rust:compile") == "oxide check --workspace --all-targets --locked"
          and tasks.get("rust:compile-release") == "RUSTFLAGS='-D warnings' oxide check --release --workspace --bins --locked"
          and tasks.get("rust:format") == "oxide fmt --all -- --check"
          and tasks.get("rust:test-600", {}).get("run") == "python3 scripts/rust-test-600.py")

    base = tempfile.mkdtemp(prefix="rust-test-600-self-check.")
    try:
        mount = os.path.join(base, "mount")
        shared = os.path.join(mount, "scratch")
        internal_parent = os.path.join(base, "internal")
        os.mkdir(mount)
        os.mkdir(shared)
        os.chmod(shared, 0o755)
        os.mkdir(internal_parent)
        probe = BOUNDED.DefaultProbe()
        device = probe.stat(mount).st_dev
        boot = probe.stat(internal_parent).st_dev
        selection = {"source": "selected-volume", "mount": mount,
                     "root_parent": os.path.join(shared, "oxide"), "device": device,
                     "volume_uuid": None, "pool": None, "pool_source": None,
                     "registration": None, "reason": None}
        roots = layout({}, selection, probe, internal_parent=internal_parent,
                       boot_device=boot)
        validation_parent = os.path.join(shared, "oxide-validation")
        check("a selected volume's validation parent is created 0700",
              stat.S_IMODE(os.stat(validation_parent).st_mode) == 0o700)
        check("the validation root is a fresh private run root",
              os.path.dirname(roots["validation"]) == validation_parent
              and os.path.basename(roots["validation"]).startswith(VALIDATION_PREFIX)
              and stat.S_IMODE(os.stat(roots["validation"]).st_mode) == 0o700)
        check("the internal root is a fresh private internal root",
              os.path.dirname(roots["internal"]) == internal_parent
              and os.path.basename(roots["internal"]).startswith(INTERNAL_PREFIX)
              and stat.S_IMODE(os.stat(roots["internal"]).st_mode) == 0o700)
        check("the two roots are distinct", roots["validation"] != roots["internal"])
        environment = {"HOME": "/home/person", "PATH": "/bin", "TMPDIR": "/tmp/elsewhere"}
        child = supervisor_environment(environment, roots)
        check("the child environment exports exactly the dual-root recipe",
              all(child[name] == roots["internal"] for name in
                  ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_TEST_INTERNAL_ROOT"))
              and all(child[name] == roots["validation"] for name in ("TMPDIR", "TMP", "TEMP")))
        check("the child environment keeps unrelated values",
              child["HOME"] == "/home/person" and child["PATH"] == "/bin")
        check("the caller's environment is not mutated",
              environment == {"HOME": "/home/person", "PATH": "/bin", "TMPDIR": "/tmp/elsewhere"})
        check("a closed run removes exactly the two per-run roots",
              cleanup(roots, 0) == "removed"
              and not os.path.lexists(roots["validation"])
              and not os.path.lexists(roots["internal"])
              and os.path.isdir(validation_parent))
        kept = layout({}, selection, probe, internal_parent=internal_parent,
                      boot_device=boot)
        check("an unclosed run keeps both roots",
              cleanup(kept, 125) == "kept"
              and os.path.isdir(kept["validation"]) and os.path.isdir(kept["internal"]))
        check("a validation temp on another device is accepted",
              not refused(distinct_devices, 1, 2))
        check("a validation temp on the boot device refuses",
              refused(distinct_devices, 2, 2))
        os.chmod(shared, 0o777)
        check("a group/other-writable shared ancestor refuses",
              refused(layout, {}, selection, probe, internal_parent=internal_parent,
                      boot_device=boot))
        os.chmod(shared, 0o755)
        wrong_device = dict(selection, device=device + 1)
        check("a validation root on another device refuses",
              refused(layout, {}, wrong_device, probe, internal_parent=internal_parent,
                      boot_device=boot))
        planted = os.path.join(shared, "oxide-validation")
        if not os.path.isdir(planted):
            os.mkdir(planted)
        os.chmod(planted, 0o755)
        existing = layout({}, selection, probe, internal_parent=internal_parent,
                          boot_device=boot)
        check("an existing 0755 validation parent is reused and its run root is 0700",
              os.path.dirname(existing["validation"]) == planted
              and stat.S_IMODE(os.stat(existing["validation"]).st_mode) == 0o700)
        for name in ("validation", "internal"):
            shutil.rmtree(existing[name], ignore_errors=True)
        os.chmod(planted, 0o775)
        check("an existing group-writable validation parent refuses and is not repaired",
              refused(layout, {}, selection, probe, internal_parent=internal_parent,
                      boot_device=boot)
              and stat.S_IMODE(os.stat(planted).st_mode) == 0o775)
        os.chmod(planted, 0o755)
        fallback = {"source": "fallback", "mount": None,
                    "root_parent": BOUNDED.FALLBACK_PARENT,
                    "reason": "no eligible selected volume", "device": None}
        fallback_roots = layout({}, fallback, probe, internal_parent=internal_parent,
                                boot_device=boot)
        check("the fallback names its reason and stays under the internal parent",
              "not selected-volume coverage" in (fallback_roots["warning"] or "")
              and os.path.dirname(fallback_roots["validation"]) == internal_parent)
        for name in ("validation", "internal"):
            shutil.rmtree(fallback_roots[name], ignore_errors=True)
    finally:
        shutil.rmtree(base, ignore_errors=True)

    check("cleanup reports a nonzero exit as kept",
          cleanup({"validation": "/nonexistent", "internal": "/nonexistent"}, 1) == "kept")
    check("an account home is not a refusal when it is the filesystem root",
          not refused(refuse_home, "/private/tmp", "/"))
    failures = [name for name, ok in checks if not ok]
    for name, ok in checks:
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
    print(f"rust:test-600 self-check: {len(checks) - len(failures)}/{len(checks)} checks ok")
    return 0 if not failures else 1


def main(argv):
    try:
        options = parse(argv)
        if options["self_check"]:
            return self_check()
        if options["plan"]:
            for line in plan_lines(options["capture_cargo"]):
                print(line)
            return 0
        return run(os.environ, capture_cargo=options["capture_cargo"])
    except Refused as refusal:
        print(f"rust-test-600: refused: {refusal}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
