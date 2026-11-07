// Prints 1 if macOS has a sandbox on the process, 0 if not: what Activity Monitor's Sandbox column reads.
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int sandbox_check(pid_t pid, const char *operation, int type);

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  printf("%d\n", sandbox_check((pid_t)atoi(argv[1]), NULL, 0));
  return 0;
}
