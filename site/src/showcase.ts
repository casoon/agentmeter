import { ansiToHtml, escapeAnsi } from '@casoon/pages-theme/ansi';
import type { ShowcaseExample } from '@casoon/pages-theme/showcase';

const files = import.meta.glob<string>('../../examples/*.ansi', {
  query: '?raw',
  import: 'default',
  eager: true,
});

const examples_ = [
  {
    slug: 'summary',
    title: 'Summary',
    file: 'summary.ansi',
    tags: ['usage', 'projects'],
    description: 'Requests, input, cache share and compactions over a period, plus the projects that drive the input.',
  },
  {
    slug: 'findings',
    title: 'Findings',
    file: 'findings.ansi',
    tags: ['diagnosis', 'tips'],
    description: 'Rules over the same data: each finding carries a number and a tip.',
  },
];

export const examples: ShowcaseExample[] = examples_.map(({ file, ...meta }) => {
  const source = files[`../../examples/${file}`];
  return {
    ...meta,
    file: `examples/${file}`,
    input: { code: escapeAnsi(source) },
    output: { html: ansiToHtml(source), kind: 'terminal' },
  };
});
