// Use the same Claude catalog as the host without advertising a model before discovery.
import { claudeModels } from '../../hoh-host/config.ts';

export function coldClaudeModel(id) {
  return claudeModels({ installed: true, version: null, versionStatus: 'unknown', auth: 'unknown', models: [{ id }] })[0];
}
