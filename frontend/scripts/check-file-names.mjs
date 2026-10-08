import { readdirSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const exceptions = new Set(['__tests__', '__mocks__', 'README.md'])
const violations = []

function visit(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (!exceptions.has(entry.name) && !/^[a-z0-9]+(?:[-.][a-z0-9]+)*$/.test(entry.name)) {
      violations.push(relative(root, path))
    }
    if (entry.isDirectory()) visit(path)
  }
}

for (const directory of ['src', 'packages']) visit(join(root, directory))
if (violations.length) {
  process.stderr.write(`前端文件和目录须使用 kebab-case：\n${violations.join('\n')}\n`)
  process.exitCode = 1
}
