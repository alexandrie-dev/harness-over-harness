// Таблица соответствий «адрес / имя модели → поставщик». Единственный файл данных для определения поставщика.
// icon — ключ значка из набора T3 (screens/shared/providerIcons.tsx); null — значка T3 нет, показывается нейтральный.
// hosts — окончания имени узла; models — начала имён моделей (регистр не важен).
export const PROVIDERS = [
  { id: 'openai', name: 'OpenAI', icon: 'openai', hosts: ['api.openai.com', 'openai.azure.com'], models: ['gpt-', 'chatgpt', 'o1', 'o3', 'o4', 'text-embedding', 'codex'] },
  { id: 'anthropic', name: 'Anthropic', icon: 'anthropic', hosts: ['api.anthropic.com'], models: [] },
  { id: 'claude', name: 'Claude', icon: 'claude', hosts: [], models: ['claude'] },
  { id: 'openrouter', name: 'OpenRouter', icon: 'openrouter', hosts: ['openrouter.ai'], models: [] },
  { id: 'ollama', name: 'Ollama', icon: 'ollama', hosts: ['ollama.com'], models: [] },
  { id: 'google', name: 'Google Gemini', icon: null, hosts: ['generativelanguage.googleapis.com', 'aiplatform.googleapis.com'], models: ['gemini', 'gemma'] },
  { id: 'mistral', name: 'Mistral', icon: null, hosts: ['api.mistral.ai'], models: ['mistral', 'mixtral', 'codestral', 'ministral', 'pixtral'] },
  { id: 'deepseek', name: 'DeepSeek', icon: null, hosts: ['api.deepseek.com'], models: ['deepseek'] },
  { id: 'xai', name: 'xAI', icon: null, hosts: ['api.x.ai'], models: ['grok'] },
  { id: 'groq', name: 'Groq', icon: null, hosts: ['api.groq.com'], models: [] },
  { id: 'together', name: 'Together AI', icon: null, hosts: ['api.together.xyz', 'api.together.ai'], models: [] },
  { id: 'qwen', name: 'Qwen', icon: null, hosts: ['dashscope.aliyuncs.com', 'dashscope-intl.aliyuncs.com'], models: ['qwen', 'qwq'] },
  { id: 'moonshot', name: 'Moonshot Kimi', icon: null, hosts: ['api.moonshot.ai', 'api.moonshot.cn'], models: ['kimi', 'moonshot'] },
  { id: 'zhipu', name: 'Z.ai GLM', icon: null, hosts: ['api.z.ai', 'open.bigmodel.cn'], models: ['glm'] },
];
// Значок для локальной модели: Ollama по его обычному порту, иначе нейтральный «компьютер».
export const OLLAMA_PORT = '11434';
