// web-ext ships no types; this is the one call scripts/lint.ts makes.
declare module "web-ext" {
  const webExt: {
    cmd: {
      lint(params: Record<string, unknown>, options?: { shouldExitProgram?: boolean }): Promise<unknown>;
    };
  };
  export default webExt;
}
