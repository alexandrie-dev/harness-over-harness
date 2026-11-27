import type { LaunchEditorInput, ProjectListEntriesInput, ProjectReadFileInput } from '@t3tools/contracts';
import type { HohEngine } from './engine.ts';
import { englishError } from './errors.ts';
import { RpcFailure } from './wire.ts';

function contains(root: string, path: string) {
  return path === root || path.startsWith(`${root}/`);
}

export function projectFileHandlers(
  engine: HohEngine,
  start: () => Promise<void>,
  roots: () => readonly string[],
) {
  const checkRoot = (cwd: string) => {
    if (!roots().includes(cwd)) throw new Error('Open the folder as a HoH project first.');
  };
  return {
    'projects.listEntries': async ({ cwd, directoryPath = '' }: ProjectListEntriesInput) => {
      try {
        await start();
        checkRoot(cwd);
        if (!engine.listProjectEntries) throw new Error('Project files are unavailable in this window.');
        return await engine.listProjectEntries({ cwd, directoryPath });
      } catch (error) {
        throw new RpcFailure({ _tag: 'ProjectListEntriesError', cwd, message: englishError(error) });
      }
    },
    'projects.readFile': async ({ cwd, relativePath, allowMissing = false }: ProjectReadFileInput) => {
      try {
        await start();
        checkRoot(cwd);
        if (!engine.readProjectFile) throw new Error('Project files are unavailable in this window.');
        return await engine.readProjectFile({ cwd, path: relativePath, allowMissing });
      } catch (error) {
        // Directory and binary replies select the corresponding T3 preview surface.
        const native = error as { failure?: unknown } | null;
        const failure = ['workspace_path_outside_root', 'resolved_path_outside_root', 'path_not_file', 'binary_file']
          .find(value => value === native?.failure) ?? 'operation_failed';
        throw new RpcFailure({ _tag: 'ProjectReadFileError', cwd, relativePath, failure, message: englishError(error) });
      }
    },
    'shell.openInEditor': async (input: LaunchEditorInput) => {
      const path = input.cwd;
      try {
        await start();
        const known = roots();
        const supplied = input.projectRoot;
        const root = supplied && known.includes(supplied) && contains(supplied, path)
          ? supplied
          : known.filter(candidate => contains(candidate, path)).sort((left, right) => right.length - left.length)[0];
        if (!root) throw new Error('Open the folder as a HoH project first.');
        if (!engine.openProjectEditor) throw new Error('Editor actions are unavailable in this window.');
        await engine.openProjectEditor({ cwd: root, path, editor: input.editor, ...(input.position ? { position: input.position } : {}) });
      } catch (error) {
        throw new RpcFailure({
          _tag: 'ExternalLauncherEditorSpawnError',
          editor: input.editor, target: path, command: input.editor, args: [], cause: englishError(error),
        });
      }
    },
  };
}
