// typescript-eslint 8.67 hard-rejects TypeScript 7: TS 7 is the native
// (Go) preview and no longer ships the classic JS API typescript-eslint
// consumes. The official workaround is to run the lint toolchain on the
// TypeScript 6 API while the application keeps TS 7 for `pnpm check`
// (tsc --noEmit). This hook rewires only the typescript-eslint packages'
// `typescript` peer to an aliased typescript@6.0.3 install; everything else
// in the dependency graph is untouched.
const TS6_ALIAS = 'npm:typescript@6.0.3';

function readPackage(pkg) {
  const isTsEslintToolchain =
    pkg.name === 'typescript-eslint' || (pkg.name ?? '').startsWith('@typescript-eslint/');
  if (isTsEslintToolchain && pkg.peerDependencies?.typescript) {
    delete pkg.peerDependencies.typescript;
    pkg.dependencies = { ...pkg.dependencies, typescript: TS6_ALIAS };
  }
  return pkg;
}

module.exports = { hooks: { readPackage } };
