# Golden fixture: sys — stdio fds, argv presence, platform presence (ticket #97).
# Env-independent: asserts shapes (fd numbers, non-empty argv/platform), not
# values, so it is identical on Linux and macOS.
def main():
    print(sys.stdin_fd())
    print(sys.stdout_fd())
    print(sys.stderr_fd())
    if len(sys.argv()) >= 1:
        print("has_argv")
    if len(sys.platform()) > 0:
        print("has_platform")
