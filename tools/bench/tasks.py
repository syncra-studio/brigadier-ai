"""Dependency-free, bounded coding fixtures and hidden deterministic graders."""
import json
import subprocess

VERSION = 1
TASKS = {
    "t1": "Add stripColor(text) to src/text.js, exporting it through src/index.js. Strip ANSI SGR escape sequences, preserve plain text and handle empty strings. Add tests.",
    "t2": "Fix splitLines(text) in src/text.js: CRLF input currently retains carriage returns. Preserve empty final lines and LF behavior. The existing tests reproduce the bug; do not edit them.",
    "t3": "Move splitLines and upper from src/text.js to src/strings.js and update src/index.js. Remove src/text.js. Preserve behavior and add tests for the public exports.",
    "t4": "Add src/cli.js: a CLI that reads all stdin, uppercases it using the library, and writes it to stdout without adding a newline. Add a test that spawns the CLI.",
    "t5": "Add two independent features: src/gradient.js exports gradient(start,end,steps), an inclusive linear numeric interpolation (steps=1 returns [start]); src/slug.js exports slug(text), lowercase ASCII letters/digits joined by single hyphens, trimming other characters from the ends. Export both from src/index.js and add tests.",
}
BASE = """import test from 'node:test';
import assert from 'node:assert/strict';
import {splitLines, upper} from '../src/index.js';
test('lines', () => assert.deepEqual(splitLines('a\\nb\\n'), ['a','b','']));
test('upper', () => assert.equal(upper('Hello'), 'HELLO'));
"""


def fixture(repo, task):
    repo.mkdir()
    (repo / "src").mkdir()
    (repo / "test").mkdir()
    (repo / "package.json").write_text(json.dumps({"type": "module", "scripts": {"test": "node --test test/*.test.js"}}))
    split = "text.split('\\n')" if task == "t2" else "text.split(/\\r?\\n/)"
    (repo / "src/text.js").write_text(f"export const splitLines = text => {split};\nexport const upper = text => text.toUpperCase();\n")
    (repo / "src/index.js").write_text("export {splitLines, upper} from './text.js';\n")
    (repo / "test/base.test.js").write_text(BASE + ("test('CRLF', () => assert.deepEqual(splitLines('a\\r\\nb\\r\\n'), ['a','b','']));\n" if task == "t2" else ""))
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=bench", "-c", "user.email=bench@localhost", "commit", "-qm", "Fixture"], check=True)


def grade(repo, task):
    suite = subprocess.run(["node", "--test", *map(str, sorted((repo / "test").glob("*.test.js")))], cwd=repo, capture_output=True, text=True, timeout=30)
    common = "import assert from 'node:assert/strict'; import * as m from './src/index.js';\n"
    hidden = {
        "t1": r"assert.equal(m.stripColor('\x1b[31mred\x1b[0m'), 'red'); assert.equal(m.stripColor('plain'), 'plain'); assert.equal(m.stripColor(''), '');",
        "t2": r"assert.deepEqual(m.splitLines('a\r\nb\n'), ['a','b','']); assert.deepEqual(m.splitLines(''), ['']);",
        "t3": "import * as s from './src/strings.js'; assert.equal(s.upper('a'), 'A'); assert.deepEqual(m.splitLines('x\\r\\n'), ['x','']);",
        "t4": "import {spawnSync} from 'node:child_process'; const r=spawnSync(process.execPath,['src/cli.js'],{input:'Hello\\nworld',encoding:'utf8'}); assert.equal(r.status,0); assert.equal(r.stdout,'HELLO\\nWORLD');",
        "t5": "assert.deepEqual(m.gradient(0,10,3),[0,5,10]); assert.deepEqual(m.gradient(7,9,1),[7]); assert.deepEqual(m.gradient(10,0,3),[10,5,0]); assert.equal(m.slug(' Hello, WORLD! '),'hello-world'); assert.equal(m.slug(''),'');",
    }[task]
    check = subprocess.run(["node", "--input-type=module", "-e", common + hidden], cwd=repo, capture_output=True, text=True, timeout=30)
    structure = (task != "t3" or not (repo / "src/text.js").exists()) and (task != "t5" or all((repo / "src" / name).is_file() for name in ("gradient.js", "slug.js")))
    unchanged = task != "t2" or (repo / "test/base.test.js").read_text() == BASE + "test('CRLF', () => assert.deepEqual(splitLines('a\\r\\nb\\r\\n'), ['a','b','']));\n"
    added_tests = task == "t2" or any(path.name != "base.test.js" or path.read_text() != BASE
                                         for path in (repo / "test").glob("*.test.js"))
    return {"passed": suite.returncode == check.returncode == 0 and structure and unchanged and added_tests,
            "suite_exit": suite.returncode, "hidden_exit": check.returncode,
            "structure": structure, "tests_unchanged": unchanged, "tests_added": added_tests,
            "detail": (suite.stdout + suite.stderr + check.stderr)[-2500:]}
