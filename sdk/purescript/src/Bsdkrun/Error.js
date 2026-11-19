export const errorName = error => error.name;
export const resultImpl = error => error.name === "CommandFailedError"
  ? {exitCode: error.exitCode, stdout: error.stdout, stderr: error.stderr} : null;
export const codeImpl = error => error.code ?? null;
