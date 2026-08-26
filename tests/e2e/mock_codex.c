#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

static void artifact_path(char *output, size_t size, const char *binary, const char *name) {
  const char *separator = strrchr(binary, '/');
  if (separator == NULL) {
    exit(20);
  }
  size_t directory_length = (size_t)(separator - binary);
  if (directory_length + 1 + strlen(name) + 1 > size) {
    exit(21);
  }
  memcpy(output, binary, directory_length);
  output[directory_length] = '/';
  strcpy(output + directory_length + 1, name);
}

static FILE *open_artifact(const char *binary, const char *name, const char *mode) {
  char path[PATH_MAX];
  artifact_path(path, sizeof(path), binary, name);
  FILE *file = fopen(path, mode);
  if (file == NULL) {
    exit(22);
  }
  return file;
}

int main(int argc, char **argv) {
  char marker_path[PATH_MAX];
  artifact_path(marker_path, sizeof(marker_path), argv[0], "confirmation-marker");
  if (access(marker_path, F_OK) != 0) {
    return 23;
  }

  FILE *stdin_capture = open_artifact(argv[0], "codex-stdin.json", "wb");
  int byte;
  while ((byte = fgetc(stdin)) != EOF) {
    if (fputc(byte, stdin_capture) == EOF) {
      return 24;
    }
  }
  if (fclose(stdin_capture) != 0) {
    return 25;
  }

  FILE *arguments = open_artifact(argv[0], "codex-argv.txt", "w");
  for (int index = 1; index < argc; index++) {
    fprintf(arguments, "%s\n", argv[index]);
  }
  fclose(arguments);

  FILE *environment = open_artifact(argv[0], "codex-env.txt", "w");
  for (char **entry = environ; *entry != NULL; entry++) {
    fprintf(environment, "%s\n", *entry);
  }
  fclose(environment);

  int count = 0;
  char count_path[PATH_MAX];
  artifact_path(count_path, sizeof(count_path), argv[0], "codex-invocations.txt");
  FILE *existing_count = fopen(count_path, "r");
  if (existing_count != NULL) {
    fscanf(existing_count, "%d", &count);
    fclose(existing_count);
  }
  existing_count = fopen(count_path, "w");
  if (existing_count == NULL) {
    return 26;
  }
  fprintf(existing_count, "%d", count + 1);
  fclose(existing_count);

  const char *output =
      "{\"schema_version\":1,\"suggestions\":[{\"action\":\"no_change\","
      "\"title\":\"Keep the measured workflow\","
      "\"rationale\":\"The sanitized aggregate does not justify a mapping change.\","
      "\"evidence\":[{\"metric\":\"sessions\",\"value\":1}],"
      "\"collision_check\":{\"checked\":true,\"conflicting_mapping_ids\":[]}}]}";
  FILE *output_capture = open_artifact(argv[0], "codex-output.json", "w");
  fputs(output, output_capture);
  fclose(output_capture);
  fputs(output, stdout);
  return 0;
}
