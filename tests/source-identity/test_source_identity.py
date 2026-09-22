"""Run Stado's real build producer against Git and immutable source inputs."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import unittest
import uuid


ROOT = Path(__file__).resolve().parents[2]
IDENTITY = "cargo:rustc-env=STADO_SOURCE_REVISION="
SOURCE_FIELDS = ("WISENT_SOURCE_COMMIT", "STADO_SOURCE_REVISION", "GIT_INDEX_FILE")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class SourceIdentityJourney(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.evidence = ROOT / ".wisent-output/source-identity-tests" / uuid.uuid4().hex
        cls.work = cls.evidence / "work"
        cls.work.mkdir(parents=True)
        cls.addClassCleanup(shutil.rmtree, cls.work)
        cls.environment = dict(os.environ)
        for field in (*SOURCE_FIELDS, "GIT_DIR", "GIT_WORK_TREE"):
            cls.environment.pop(field, None)
        cls.environment["GIT_OPTIONAL_LOCKS"] = "0"
        cls.command_count = 0
        cls.head = cls.checked(["git", "rev-parse", "HEAD"]).strip()
        cls.previous = cls.checked(["git", "rev-parse", "HEAD^"]).strip()
        cls.operator_index = ROOT / cls.checked(["git", "rev-parse", "--git-path", "index"]).strip()
        cls.index_digest = digest(cls.operator_index)
        cls.addClassCleanup(cls.check_operator_index)
        inputs = [ROOT / "stado-rs/build.rs", Path(__file__).resolve()]
        for directory in ("deploy/join", "stado-rs/src/deploy/host_storage_reconcile_program"):
            inputs.extend(path for path in (ROOT / directory).iterdir() if path.is_file())
        (cls.evidence / "source.json").write_text(json.dumps({
            "revision": cls.head,
            "files": {str(path.relative_to(ROOT)): digest(path) for path in sorted(inputs)},
        }, indent=2) + "\n")
        patch = cls.checked(["git", "diff", "--binary", "HEAD", "--", "stado-rs/build.rs"])
        (cls.evidence / "working.patch").write_text(patch)
        cls.binary = cls.work / "build-script-build"
        cls.checked(["rustc", "--edition=2021", "stado-rs/build.rs", "-o", str(cls.binary)])
        cls.private_index = cls.work / "index"
        indexed = {**cls.environment, "GIT_INDEX_FILE": str(cls.private_index)}
        cls.checked(["git", "read-tree", "HEAD"], env=indexed)
        cls.checked(["git", "update-index", "--force-remove", "stado-rs/build.rs"], env=indexed)
        # This is a real private-index change, not a changed operator file or index.
        status = cls.checked(["git", "status", "--porcelain", "--", "stado-rs/build.rs"], env=indexed)
        if not status.strip():
            raise AssertionError("the private source view did not become dirty")
        cls.archive = cls.work / "archive"
        cls.archive.mkdir()
        archive = cls.work / "source.tar"
        cls.checked([
            "git", "archive", "--format=tar", f"--output={archive}", cls.head,
            "deploy/join", "stado-rs/src/deploy/host_storage_reconcile_program",
        ])
        with tarfile.open(archive) as source:
            source.extractall(cls.archive, filter="data")
        (cls.evidence / "archive.json").write_text(json.dumps({
            "revision": cls.head, "sha256": digest(archive),
            "git_metadata_present": (cls.archive / ".git").exists(),
        }, indent=2) + "\n")
        print(f"source identity evidence: {cls.evidence}", flush=True)

    @classmethod
    def check_operator_index(cls) -> None:
        if digest(cls.operator_index) != cls.index_digest:
            raise AssertionError("the operator Git index changed during the journey")

    @classmethod
    def run_command(cls, argv: list[str], *, cwd: Path = ROOT, env: dict | None = None):
        cls.command_count += 1
        selected = cls.environment if env is None else env
        result = subprocess.run(argv, cwd=cwd, env=selected, text=True, capture_output=True)
        record = cls.evidence / f"command-{cls.command_count}"
        record.mkdir()
        (record / "command.json").write_text(json.dumps({
            "argv": argv, "cwd": str(cwd), "exit_status": result.returncode,
            "source_environment": {key: selected[key] for key in SOURCE_FIELDS if key in selected},
        }, indent=2) + "\n")
        (record / "stdout.log").write_text(result.stdout)
        (record / "stderr.log").write_text(result.stderr)
        return result

    @classmethod
    def checked(cls, argv: list[str], **kwargs) -> str:
        result = cls.run_command(argv, **kwargs)
        if result.returncode != 0:
            raise AssertionError(f"{argv}: exit {result.returncode}\n{result.stderr}")
        return result.stdout

    def produce(self, commit: str, *, archive: bool = False, explicit: str | None = None):
        output = self.work / self.id().rsplit(".", 1)[-1]
        output.mkdir()
        environment = {
            **self.environment, "WISENT_SOURCE_COMMIT": commit, "OUT_DIR": str(output),
        }
        if explicit is not None:
            environment["STADO_SOURCE_REVISION"] = explicit
        if not archive:
            environment["GIT_INDEX_FILE"] = str(self.private_index)
        directory = (self.archive if archive else ROOT) / "stado-rs"
        return self.run_command([str(self.binary)], cwd=directory, env=environment)

    def assert_refused(self, result) -> None:
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertNotIn(IDENTITY, result.stdout)

    def test_owner_local_commit_preserves_the_measured_dirty_state(self) -> None:
        result = self.produce(self.head)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"{IDENTITY}{self.head}-dirty", result.stdout.splitlines())

    def test_owner_local_commit_cannot_relabel_another_checkout(self) -> None:
        result = self.produce(self.previous)
        self.assert_refused(result)
        self.assertIn(self.previous, result.stderr)
        self.assertIn(self.head, result.stderr)

    def test_archive_keeps_its_commit_without_inheriting_parent_git_state(self) -> None:
        result = self.produce(self.head, archive=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"{IDENTITY}{self.head}", result.stdout.splitlines())

    def test_archive_refuses_an_inherited_conflicting_revision(self) -> None:
        result = self.produce(self.head, archive=True, explicit=self.previous)
        self.assert_refused(result)
        self.assertIn(self.previous, result.stderr)
        self.assertIn(self.head, result.stderr)

    def test_archive_does_not_accept_a_dirty_value_as_an_immutable_commit(self) -> None:
        result = self.produce(f"{self.head}-dirty", archive=True)
        self.assert_refused(result)
        self.assertIn("WISENT_SOURCE_COMMIT", result.stderr)


if __name__ == "__main__":
    unittest.main()
