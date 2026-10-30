// @ts-check
import casoonPages from '@casoon/pages-theme';
import { defineConfig } from 'astro/config';

// Project page: https://casoon.github.io/agentmeter/ — `base` is the GitHub Pages path.
export default defineConfig({
  site: 'https://casoon.github.io/agentmeter',
  base: '/agentmeter/',
  integrations: [
    casoonPages({
      name: 'agentmeter',
      description:
        'Reads the local session logs of Claude Code and Codex and explains usage with evidence: context size, cache rebuilds, session length.',
      tagline: 'Explains coding-agent usage',
      repo: 'casoon/agentmeter',
      license: 'MIT',
      docsGroups: {
        '': 'The project',
        reference: 'Reference',
      },
    }),
  ],
});
