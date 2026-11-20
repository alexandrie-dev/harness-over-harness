import assert from 'node:assert/strict';
import { test } from 'node:test';
import { addressScope, detectProvider } from '../../src/lib/providers.mjs';
import { PROVIDERS } from '../../src/lib/providers.data.mjs';

test('this computer and the private network are a local model, by the address alone', () => {
  for (const url of ['http://localhost:8000/v1', 'http://127.0.0.1:8000', 'http://[::1]:8000/v1']) {
    const found = detectProvider(url, 'qwen3-32b');
    assert.equal(found.scope, 'loopback', url); assert.equal(found.label, 'Локальная модель'); assert.equal(found.icon, 'local');
  }
  for (const url of ['http://192.168.1.5:8000/v1', 'http://10.0.0.7/v1', 'http://172.20.3.4:9000', 'http://gpu-box:8000/v1', 'http://spark.local/v1', 'http://100.101.1.1:8000']) {
    const found = detectProvider(url, 'local');
    assert.equal(found.scope, 'private', url); assert.equal(found.label, 'Локальная модель в сети');
  }
  assert.equal(addressScope('172.32.0.1'), ''); assert.equal(addressScope('8.8.8.8'), '');
  assert.equal(detectProvider('http://localhost:11434/v1', 'llama3').icon, 'ollama');
});

test('known cloud addresses and model names give the service, its icon and the model as the label', () => {
  assert.deepEqual(detectProvider('https://api.openai.com/v1', 'gpt-5'), { id: 'openai', name: 'OpenAI', icon: 'openai', scope: 'cloud', label: 'gpt-5' });
  assert.equal(detectProvider('https://api.anthropic.com/v1', '').icon, 'anthropic');
  assert.equal(detectProvider('https://api.anthropic.com/v1', 'claude-fable-5-2').icon, 'claude');
  assert.equal(detectProvider('https://openrouter.ai/api/v1', 'anthropic/claude-sonnet-5-5').id, 'openrouter');
  // an unknown relay is told apart by the model name
  const relay = detectProvider('https://relay.example.org/v1', 'claude-opus-5-5');
  assert.equal(relay.id, 'claude'); assert.equal(relay.scope, 'cloud');
  // services without a T3 icon are named, and the icon is neutral
  assert.deepEqual([detectProvider('https://api.deepseek.com', 'deepseek-chat').name, detectProvider('https://api.deepseek.com', 'deepseek-chat').icon], ['DeepSeek', null]);
  assert.equal(detectProvider('https://generativelanguage.googleapis.com/v1beta', 'gemini-3-pro').name, 'Google Gemini');
});

test('an unknown address is neutral and never throws on garbage', () => {
  const found = detectProvider('https://llm.example.org/v1', 'house-model-7');
  assert.deepEqual([found.id, found.icon, found.scope, found.label], ['unknown', null, 'cloud', 'house-model-7']);
  for (const bad of ['', 'not a url', undefined, null, 'https://']) assert.equal(detectProvider(bad, '').id, 'unknown');
  assert.equal(detectProvider('https://api.openai.com.evil.example/v1', 'x').id, 'unknown', 'the name of the service inside a longer name is not the service');
});

test('the table is one file of plain data with unique ids', () => {
  assert.equal(new Set(PROVIDERS.map(p => p.id)).size, PROVIDERS.length);
  for (const p of PROVIDERS) assert(Array.isArray(p.hosts) && Array.isArray(p.models), p.id);
});
