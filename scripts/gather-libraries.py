#!/usr/bin/env python3
"""Copy every non-system library an application needs into its bundle.

    gather-libraries.py <App.app> [extra library ...]

Starting from the executable (and any extra libraries named, for ones loaded at run time
rather than linked), follows the chain of dependencies, copies each library into
Contents/Frameworks and rewrites the references so that they are found there. Libraries that
are part of macOS stay where they are.
"""

import os
import shutil
import subprocess
import sys

SYSTEM = ("/usr/lib/", "/System/")
SEARCH = ["/opt/homebrew/lib", "/usr/local/lib"]


def dependencies(path):
    """The install names a binary refers to, without its own."""
    out = subprocess.run(["otool", "-L", path], capture_output=True, text=True, check=True).stdout
    names = [line.strip().split(" (")[0] for line in out.splitlines()[1:] if line.strip()]
    own = subprocess.run(["otool", "-D", path], capture_output=True, text=True).stdout.splitlines()[1:]
    return [n for n in names if n not in own]


def locate(name, referrer):
    """The file on disk behind an install name, or None for a system library."""
    if name.startswith(SYSTEM):
        return None
    if name.startswith("@"):
        leaf = name.split("/", 1)[1]
        places = [os.path.dirname(os.path.realpath(referrer))] + SEARCH
        for place in places:
            candidate = os.path.join(place, leaf)
            if os.path.exists(candidate):
                return candidate
            candidate = os.path.join(place, os.path.basename(leaf))
            if os.path.exists(candidate):
                return candidate
        # @rpath/libswift*.dylib and the like live in the system.
        return None
    return name if os.path.exists(name) else None


def main():
    app = sys.argv[1]
    frameworks = os.path.join(app, "Contents", "Frameworks")
    macos = os.path.join(app, "Contents", "MacOS")
    os.makedirs(frameworks, exist_ok=True)
    executable = os.path.join(macos, sorted(os.listdir(macos))[0])

    # binary in the bundle -> [(install name it uses, bundled file name)]
    rewrites = {executable: []}
    # file name in the bundle -> where it came from
    copied = {}
    queue = [(executable, executable)]

    for extra in sys.argv[2:]:
        leaf = os.path.basename(extra)
        target = os.path.join(frameworks, leaf)
        shutil.copyfile(os.path.realpath(extra), target)
        os.chmod(target, 0o755)
        copied[leaf] = extra
        rewrites[target] = []
        queue.append((target, extra))

    while queue:
        binary, origin = queue.pop()
        for name in dependencies(binary):
            source = locate(name, origin)
            if source is None:
                continue
            leaf = os.path.basename(name)
            target = os.path.join(frameworks, leaf)
            rewrites[binary].append((name, leaf))
            if leaf not in copied:
                shutil.copyfile(os.path.realpath(source), target)
                os.chmod(target, 0o755)
                copied[leaf] = source
                rewrites[target] = []
                queue.append((target, source))

    for binary, changes in rewrites.items():
        command = ["install_name_tool"]
        if binary != executable:
            command += ["-id", "@rpath/" + os.path.basename(binary)]
        for old, leaf in changes:
            command += ["-change", old, "@rpath/" + leaf]
        if len(command) > 1:
            subprocess.run(command + [binary], check=True, capture_output=True)
    subprocess.run(
        ["install_name_tool", "-add_rpath", "@executable_path/../Frameworks", executable],
        check=True, capture_output=True,
    )

    size = sum(os.path.getsize(os.path.join(frameworks, f)) for f in copied)
    print(f"{len(copied)} libraries, {size / 1e6:.0f} MB")

    # Nothing may still point outside the bundle and the system.
    stray = []
    for binary in rewrites:
        stray += [f"{os.path.basename(binary)} -> {n}" for n in dependencies(binary)
                  if not n.startswith(SYSTEM) and not n.startswith("@rpath/")]
    if stray:
        sys.exit("still referring outside the bundle:\n  " + "\n  ".join(stray))


if __name__ == "__main__":
    main()
