'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');

const SOURCE_EXTENSIONS = new Set([
  '.cjs',
  '.js',
  '.jsx',
  '.mjs',
  '.mts',
  '.cts',
  '.ts',
  '.tsx',
  '.vue',
]);
const MODULE_EXTENSIONS = ['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs', '.vue', '.json'];
let serializedOutput;

function main() {
  let input;
  try {
    input = JSON.parse(fs.readFileSync(0, 'utf8'));
  } catch (error) {
    write({
      usedIds: [],
      coveredIds: [],
      unknownIds: [],
      vueScripts: [],
      diagnostics: [diagnostic('invalid-json', `invalid JSON input: ${error.message}`)],
    });
    return;
  }

  try {
    if (input && input.mode === 'prepare') {
      write(prepare(input));
    } else if (input && input.mode === 'references') {
      write(references(input));
    } else {
      write({
        usedIds: [],
        coveredIds: [],
        unknownIds: [],
        vueScripts: [],
        diagnostics: [diagnostic('invalid-mode', 'mode must be "prepare" or "references"')],
      });
    }
  } catch (error) {
    write({
      usedIds: [],
      coveredIds: [],
      unknownIds: [],
      vueScripts: [],
      diagnostics: [diagnostic('runtime', errorMessage(error))],
    });
  }
}

function write(value) {
  serializedOutput = `${JSON.stringify(value)}\n`;
}

function prepare(input) {
  const root = resolveRoot(input.root);
  const compiler = loadCompilerSfc(root);
  const diagnostics = [];
  const vueScripts = [];

  for (const rawFile of strings(input.vueFiles)) {
    const filePath = resolveFile(root, rawFile);
    let source;
    try {
      source = fs.readFileSync(filePath, 'utf8');
    } catch (error) {
      diagnostics.push(diagnostic('read-file', `cannot read Vue file: ${errorMessage(error)}`, filePath));
      continue;
    }

    let parsed;
    try {
      parsed = parseVueSfc(compiler, source, filePath);
    } catch (error) {
      diagnostics.push(diagnostic('vue-parse', `cannot parse Vue file: ${errorMessage(error)}`, filePath));
      continue;
    }
    for (const error of parsed.errors) {
      diagnostics.push(diagnostic('vue-parse', formatSfcError(error), filePath));
    }

    const blocks = [];
    for (const block of [parsed.descriptor.script, parsed.descriptor.scriptSetup]) {
      if (!block) {
        continue;
      }
      blocks.push({
        content: block.content,
        offset: Buffer.byteLength(source.slice(0, vueBlockStart(block, source)), 'utf8'),
        lang: block.attrs?.lang || block.lang || 'js',
      });
    }
    vueScripts.push({ path: relativePath(root, filePath), blocks });
  }

  return { vueScripts, diagnostics };
}

function parseVueSfc(compiler, source, filePath) {
  if (typeof compiler.parse === 'function') {
    let direct;
    try {
      direct = compiler.parse(source, { filename: filePath });
      if (direct?.descriptor) {
        return { descriptor: direct.descriptor, errors: direct.errors || [] };
      }
    } catch {
      // Vue 2.7 compiler-sfc uses an object argument; retry below.
    }
    try {
      const descriptor = compiler.parse({ source, filename: filePath });
      if (descriptor) {
        return { descriptor, errors: descriptor.errors || [] };
      }
    } catch {
      // Fall through to a direct descriptor or parseComponent.
    }
    if (direct) {
      return { descriptor: direct, errors: direct.errors || [] };
    }
  }
  if (typeof compiler.parseComponent === 'function') {
    const descriptor = compiler.parseComponent(source, { pad: false });
    return { descriptor, errors: descriptor.errors || [] };
  }
  throw new Error('Vue SFC compiler does not expose parse or parseComponent');
}

function vueBlockStart(block, source) {
  const start = block.loc?.start?.offset ?? block.start;
  if (Number.isInteger(start) && start >= 0) {
    return start;
  }
  const fallback = source.indexOf(block.content);
  return fallback < 0 ? 0 : fallback;
}

function references(input) {
  const root = resolveRoot(input.root);
  const candidates = normalizeCandidates(root, input.candidates);
  const allowedPaths = new Set(strings(input.sourceFiles).map(file => normalizePath(resolveFile(root, file))));
  const diagnostics = [];
  const usedIds = new Set();
  const unknownIds = new Set();
  const unknownReasons = new Map();
  const boundarySources = [];
  const edges = [];
  if (!candidates.length) {
    return { usedIds: [], coveredIds: [], unknownIds: [], unknownReasons: [], boundarySources, edges, diagnostics };
  }

  const project = createProject(root, input.vueFiles, input.sourceFiles, candidates, diagnostics);
  if (!project) {
    return { usedIds: [], coveredIds: [], unknownIds: [], unknownReasons: [], boundarySources, edges, diagnostics };
  }

  const symbolCandidates = candidates.filter(candidate => candidate.kind !== 'file');
  const candidateNames = new Set(symbolCandidates.map(candidate => candidate.name));
  const candidateIndex = indexCandidates(symbolCandidates);
  const symbolCache = new Map();
  const sourceFiles = project.program
    .getSourceFiles()
    .filter(sourceFile => allowedPaths.has(sourcePathForCandidate(sourceFile.fileName)));
  const fileCandidates = candidates.filter(candidate => isLikelyFileCandidate(candidate));
  const fileIdsByPath = new Map(fileCandidates.map(candidate => [candidate.path, candidate.id]));
  const aliasNames = collectAliasNames(sourceFiles, project.ts);
  const methodCandidatesByName = collectMethodCandidates(candidates);
  const sourcePaths = new Set(sourceFiles.map(sourceFile => sourcePathForCandidate(sourceFile.fileName)));
  const coveredIds = [...new Set(candidates
    .filter(candidate => sourcePaths.has(candidate.path)
      && (!candidate.path.toLowerCase().endsWith('.vue') || project.vueSemantic))
    .map(candidate => candidate.id))].sort();
  if (!project.vueSemantic) {
    for (const candidate of candidates.filter(candidate => candidate.path.toLowerCase().endsWith('.vue'))) {
      markUnknown(unknownIds, unknownReasons, candidate.id, 'vue-semantic-unavailable');
    }
  }

  for (const sourceFile of sourceFiles) {
    if (!SOURCE_EXTENSIONS.has(path.extname(sourceFile.fileName).toLowerCase())
      || isTestSource(relativePath(root, sourceFile.fileName))) {
      continue;
    }
    scanModuleEdges(
      sourceFile,
      project,
      root,
      fileCandidates,
      candidateIndex,
      fileIdsByPath,
      symbolCache,
      usedIds,
      unknownIds,
      unknownReasons,
      boundarySources,
      edges,
      diagnostics,
    );
    scanIdentifiers(
      sourceFile,
      project,
      root,
      candidateNames,
      aliasNames.get(normalizePath(sourceFile.fileName)) || new Set(),
      candidateIndex,
      fileIdsByPath,
      symbolCache,
      methodCandidatesByName,
      usedIds,
      unknownIds,
      unknownReasons,
      edges,
    );
  }

  return {
    usedIds: [...usedIds].sort(),
    coveredIds,
    unknownIds: [...unknownIds].filter(id => !usedIds.has(id)).sort(),
    unknownReasons: [...unknownReasons]
      .filter(([id]) => !usedIds.has(id))
      .map(([id, reason]) => ({ id, reason }))
      .sort((left, right) => left.id.localeCompare(right.id) || left.reason.localeCompare(right.reason)),
    boundarySources: boundarySources
      .sort((left, right) => left.source.localeCompare(right.source) || left.reason.localeCompare(right.reason)),
    edges: uniqueEdges(edges),
    diagnostics,
  };
}

function createProject(root, rawVueFiles, rawSourceFiles, candidates, diagnostics) {
  const dependencies = loadVueDependencies(root);
  const ts = dependencies.typescript;
  const core = dependencies.languageCore;
  const volar = dependencies.volarTypescript;
  const volarLanguageCore = dependencies.volarLanguageCore;
  if (!ts) {
    diagnostics.push(diagnostic('missing-typescript', 'TypeScript is not installed in target project'));
    return undefined;
  }

  const configPath = ts.findConfigFile(root, ts.sys.fileExists, 'tsconfig.json')
    || ts.findConfigFile(root, ts.sys.fileExists, 'jsconfig.json');
  let options;
  let fileNames = [];
  if (configPath) {
    const config = ts.readConfigFile(configPath, ts.sys.readFile);
    if (config.error) {
      diagnostics.push(diagnostic('tsconfig', formatTsDiagnostic(config.error, ts), configPath));
    } else {
      const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, root, undefined, configPath);
      options = parsed.options;
      fileNames = parsed.fileNames;
      diagnostics.push(...(parsed.errors || []).map(error => diagnostic('tsconfig', formatTsDiagnostic(error, ts), configPath)));
    }
  }
  options ||= {
    target: ts.ScriptTarget.ESNext,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    jsx: ts.JsxEmit.Preserve,
    skipLibCheck: true,
  };
  options.allowNonTsExtensions = true;
  options.allowArbitraryExtensions = true;
  options.allowJs = true;
  options.noEmit = true;
  options.configFilePath ||= configPath;

  let vueOptions;
  try {
    vueOptions = core && (configPath
      ? core.createParsedCommandLine(ts, ts.sys, configPath).vueOptions
      : core.getDefaultCompilerOptions());
  } catch (error) {
    diagnostics.push(diagnostic('vue-options', `cannot load Vue compiler options: ${errorMessage(error)}`, configPath));
    vueOptions = core ? core.getDefaultCompilerOptions() : undefined;
  }

  const candidateSourcePaths = candidates
    .map(candidate => candidate.path)
    .filter(file => SOURCE_EXTENSIONS.has(path.extname(file).toLowerCase()));
  const sourcePaths = strings(rawSourceFiles)
    .map(file => resolveFile(root, file))
    .filter(file => SOURCE_EXTENSIONS.has(path.extname(file).toLowerCase()));
  const allowedPaths = new Set(sourcePaths.map(normalizePath));
  const vueFiles = uniquePaths([
    ...strings(rawVueFiles).map(file => resolveFile(root, file)),
    ...candidateSourcePaths.filter(file => file.toLowerCase().endsWith('.vue')),
  ]);
  const candidateRoots = candidateSourcePaths.filter(file => ts.sys.fileExists(file));
  const existingVueFiles = vueFiles.filter(file => ts.sys.fileExists(file));
  const rootNames = uniquePaths([
    ...fileNames.filter(file => allowedPaths.has(normalizePath(file)) && !file.toLowerCase().endsWith('.vue') && !isTestSource(relativePath(root, file))),
    ...sourcePaths,
    ...candidateRoots,
    ...existingVueFiles,
  ]).filter(file => ts.sys.fileExists(file) && !isTestSource(relativePath(root, file)));
  let program;
  let vueSemantic = false;
  if (core && volar && volarLanguageCore) {
    try {
      program = createVolarProgram(
        root,
        rootNames,
        options,
        vueOptions,
        ts,
        core,
        volar,
        volarLanguageCore,
      );
      vueSemantic = Boolean(program);
    } catch (error) {
      diagnostics.push(diagnostic('vue-program', `cannot create Vue TypeScript program: ${errorMessage(error)}`));
    }
  } else if (vueFiles.length) {
    diagnostics.push(diagnostic('missing-vue-semantic', 'vue-tsc is not installed; Vue template references were skipped'));
  }
  if (!program) {
    try {
      const host = ts.createCompilerHost(options, true);
      host.getCurrentDirectory = () => root;
      program = ts.createProgram({ rootNames, options, host });
    } catch (error) {
      diagnostics.push(diagnostic('program', `cannot create TypeScript program: ${errorMessage(error)}`));
      return undefined;
    }
  }
  return {
    ts,
    core,
    checker: program.getTypeChecker(),
    options,
    program,
    vueSemantic,
  };
}

function createVolarProgram(root, rootNames, options, vueOptions, ts, vueCore, volarTypescript, volarLanguageCore) {
  const languagePlugin = vueCore.createVueLanguagePlugin(ts, options, vueOptions, id => id);
  const scriptRegistry = new volarLanguageCore.FileMap(ts.sys.useCaseSensitiveFileNames);
  let language;
  language = volarLanguageCore.createLanguage(
    [languagePlugin, { getLanguageId: volarTypescript.resolveFileLanguageId }],
    scriptRegistry,
    (fileName, includeFsFiles) => {
      if (includeFsFiles && !scriptRegistry.has(fileName)) {
        const source = ts.sys.readFile(fileName);
        if (source !== undefined) {
          language.scripts.set(fileName, ts.ScriptSnapshot.fromString(source));
        }
      }
    },
  );
  const projectHost = {
    getCurrentDirectory: () => root,
    getCompilationSettings: () => options,
    getProjectReferences: () => undefined,
    getScriptFileNames: () => rootNames,
    getProjectVersion: () => String(rootNames.length),
    getLocalizedDiagnosticMessages: () => undefined,
  };
  const { languageServiceHost } = volarTypescript.createLanguageServiceHost(
    ts,
    ts.sys,
    language,
    id => id,
    projectHost,
  );
  return ts.createLanguageService(languageServiceHost).getProgram();
}

function scanIdentifiers(sourceFile, project, root, candidateNames, aliasNames, candidateIndex, fileIdsByPath, symbolCache, methodCandidatesByName, usedIds, unknownIds, unknownReasons, edges) {
  const { ts, checker } = project;
  const sourcePath = sourcePathForCandidate(sourceFile.fileName);
  const isVue = sourcePath.toLowerCase().endsWith('.vue');
  function visit(node) {
    const name = semanticIdentifierName(node, ts);
    if (name
      && (candidateNames.has(name) || aliasNames.has(name))
      && isReferenceIdentifier(node, ts)
      && !isCallTarget(node, ts)
      && !isWriteOnlyIdentifier(node, ts)) {
      const symbol = resolveSymbol(checker, node, ts);
      if (!symbol || isSelfReference(node, symbol, ts)) {
        ts.forEachChild(node, visit);
        return;
      }
      const resolution = markSymbol(
        symbol,
        root,
        sourcePathForCandidate(sourceFile.fileName),
        candidateIndex,
        fileIdsByPath,
        usedIds,
        ts,
        symbolCache,
      );
      const start = safeStart(node);
      const location = originalCallLocation(sourceFile, sourcePath, start);
      const relativeSourcePath = relativePath(root, sourcePath);
      const source = moduleEdgeSource(
        node,
        sourcePath,
        relativeSourcePath,
        candidateIndex,
        ts,
      ) || (isVue ? `file::${relativeSourcePath}` : undefined);
      if (!source) {
        ts.forEachChild(node, visit);
        return;
      }
      const mode = isTypeReference(node, ts) ? 'type' : 'runtime';
      for (const target of resolution.ids) {
        edges.push({
          source,
          target,
          kind: 'reference',
          path: relativeSourcePath,
          start,
          line: location.line,
          column: location.column,
          confidence: resolution.ids.length === 1 ? 'exact' : 'potential',
          mode,
          provenance: isVue ? 'volar' : 'typescript',
        });
      }
      for (const file of resolution.files || []) {
        if (file.path === sourcePath) {
          continue;
        }
        edges.push({
          source,
          target: file.id,
          kind: 'reference',
          path: relativeSourcePath,
          start,
          line: location.line,
          column: location.column,
          confidence: 'exact',
          mode,
          provenance: isVue ? 'volar' : 'typescript',
        });
      }
      if (isMethodReference(node, ts)
        && (!resolution.matched || !resolution.implementation)) {
        for (const candidate of methodCandidatesByName.get(name) || []) {
          if (!usedIds.has(candidate.id)) {
            markUnknown(unknownIds, unknownReasons, candidate.id, 'runtime-dispatch-ambiguous');
          }
        }
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(sourceFile);
}

function semanticIdentifierName(node, ts) {
  if (ts.isIdentifier(node)) {
    return node.text;
  }
  if (ts.isPrivateIdentifier(node)) {
    return node.text.replace(/^#/, '');
  }
  return undefined;
}

function isWriteOnlyIdentifier(node, ts) {
  let target = node;
  if (ts.isPropertyAccessExpression(node.parent) && node.parent.name === node) {
    target = node.parent;
  }
  const parent = target.parent;
  return !!parent
    && ts.isBinaryExpression(parent)
    && parent.left === target
    && parent.operatorToken.kind === ts.SyntaxKind.EqualsToken;
}

function collectAliasNames(sourceFiles, ts) {
  const namesByFile = new Map();
  for (const sourceFile of sourceFiles) {
    const names = new Set();
    function visit(node) {
      if (node.kind === ts.SyntaxKind.ImportDeclaration && node.importClause) {
        const clause = node.importClause;
        if (clause.name) {
          names.add(clause.name.text);
        }
        if (clause.namedBindings) {
          if (ts.isNamespaceImport(clause.namedBindings)) {
            names.add(clause.namedBindings.name.text);
          } else {
            for (const element of clause.namedBindings.elements) {
              names.add(element.name.text);
            }
          }
        }
      }
      ts.forEachChild(node, visit);
    }
    visit(sourceFile);
    namesByFile.set(normalizePath(sourceFile.fileName), names);
  }
  return namesByFile;
}

function collectMethodCandidates(candidates) {
  const methods = new Map();
  for (const candidate of candidates) {
    if (candidate.kind === 'method') {
      addMethodCandidate(methods, candidate);
    }
  }
  return methods;
}

function addMethodCandidate(methods, candidate) {
  const list = methods.get(candidate.name) || [];
  list.push(candidate);
  methods.set(candidate.name, list);
}

function isMethodReference(node, ts) {
  const parent = node.parent;
  return ts.isPropertyAccessExpression(parent) && parent.name === node;
}

function isCallTarget(node, ts) {
  const parent = node.parent;
  if (ts.isCallExpression(parent) || ts.isNewExpression(parent)) {
    return parent.expression === node;
  }
  if (ts.isPropertyAccessExpression(parent) && parent.name === node) {
    const call = parent.parent;
    return (ts.isCallExpression(call) || ts.isNewExpression(call)) && call.expression === parent;
  }
  return false;
}

function isSelfReference(node, symbol, ts) {
  const owner = callableOwner(node, ts);
  if (!owner) {
    return false;
  }
  const declarations = symbol.declarations || (symbol.valueDeclaration ? [symbol.valueDeclaration] : []);
  return declarations.some(declaration => declaration === owner);
}

function callableOwner(node, ts) {
  let current = node.parent;
  while (current) {
    if (ts.isFunctionDeclaration(current)
      || ts.isMethodDeclaration(current)
      || ts.isGetAccessorDeclaration(current)
      || ts.isSetAccessorDeclaration(current)
      || ts.isConstructorDeclaration(current)) {
      return current;
    }
    if (ts.isArrowFunction(current) || ts.isFunctionExpression(current)) {
      const parent = current.parent;
      if (ts.isVariableDeclaration(parent) && parent.initializer === current) {
        return parent;
      }
      return current;
    }
    if (ts.isSourceFile(current)) {
      return undefined;
    }
    current = current.parent;
  }
  return undefined;
}

function resolveSymbol(checker, identifier, ts) {
  let symbol = checker.getSymbolAtLocation(identifier);
  if (symbol
    && identifier.parent
    && ts.isShorthandPropertyAssignment(identifier.parent)
    && checker.getShorthandAssignmentValueSymbol) {
    symbol = checker.getShorthandAssignmentValueSymbol(identifier.parent) || symbol;
  }
  if (symbol && symbol.flags & ts.SymbolFlags.Alias) {
    try {
      symbol = checker.getAliasedSymbol(symbol);
    } catch {
      return undefined;
    }
  }
  return symbol;
}

function scanModuleEdges(sourceFile, project, root, fileCandidates, candidateIndex, fileIdsByPath, symbolCache, usedIds, unknownIds, unknownReasons, boundarySources, edges, diagnostics) {
  const { ts, checker, options } = project;
  function visit(node) {
    const exportAssignment = commonJsExportAssignment(node, ts);
    if (exportAssignment) {
      if (exportAssignment.dynamic) {
        markRuntimeBoundary(node, sourceFile, root, fileCandidates, candidateIndex, unknownIds, unknownReasons, boundarySources, ts, 'commonjs-export-incomplete');
        diagnostics.push(diagnostic(
          'commonjs-export-incomplete',
          'CommonJS export target is not statically resolvable',
          relativePath(root, sourceFile.fileName),
          sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
        ));
      } else {
        recordCommonJsExportEdges(
          node,
          exportAssignment.values,
          sourceFile,
          root,
          checker,
          candidateIndex,
          fileIdsByPath,
          ts,
          symbolCache,
          edges,
        );
      }
    }
    if (node.kind === ts.SyntaxKind.ImportDeclaration) {
      if (isVirtualOrLoaderSpecifier(node.moduleSpecifier.text)) {
        markRuntimeBoundary(node, sourceFile, root, fileCandidates, candidateIndex, unknownIds, unknownReasons, boundarySources, ts, 'virtual-module-boundary');
        diagnostics.push(diagnostic(
          'virtual-module-boundary',
          `module ${JSON.stringify(node.moduleSpecifier.text)} is resolved by an unsupported virtual module or loader`,
          relativePath(root, sourceFile.fileName),
          sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
        ));
      }
      else {
        markModuleSpecifier(
          node.moduleSpecifier,
          sourceFile,
          root,
          fileCandidates,
          usedIds,
          options,
          candidateIndex,
          unknownIds,
          unknownReasons,
          ts,
          edges,
          importDeclarationMode(node, ts),
          'import',
        );
      }
    } else if (node.kind === ts.SyntaxKind.ExportDeclaration) {
      // Re-exports are deliberately not file-consumer evidence.
    } else if (node.kind === ts.SyntaxKind.CallExpression && isImportMetaGlob(node, ts)) {
      const pattern = node.arguments[0];
      if (pattern && ts.isStringLiteralLike(pattern)) {
        markGlobFiles(
          pattern.text,
          sourceFile.fileName,
          root,
          options,
          fileCandidates,
          usedIds,
          candidateIndex,
          unknownIds,
          edges,
          sourceFile,
          node,
          ts,
        );
      } else {
        markRuntimeBoundary(
          node,
          sourceFile,
          root,
          fileCandidates,
          candidateIndex,
          unknownIds,
          unknownReasons,
          boundarySources,
          ts,
          'glob-boundary',
        );
        diagnostics.push(diagnostic(
          'import-meta-glob',
          'import.meta.glob pattern is unresolved',
          relativePath(root, sourceFile.fileName),
          sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
        ));
      }
    } else if (node.kind === ts.SyntaxKind.ImportEqualsDeclaration) {
      if (node.moduleReference && node.moduleReference.expression) {
        markModuleSpecifier(
          node.moduleReference.expression,
          sourceFile,
          root,
          fileCandidates,
          usedIds,
          options,
          candidateIndex,
          unknownIds,
          unknownReasons,
          ts,
          edges,
          node.isTypeOnly ? 'type' : 'runtime',
          'import',
        );
      }
    } else if (node.kind === ts.SyntaxKind.CallExpression && isImportCall(node, ts)) {
      const argument = node.arguments[0];
      if (argument && ts.isStringLiteralLike(argument)) {
        if (isVirtualOrLoaderSpecifier(argument.text)) {
          markRuntimeBoundary(node, sourceFile, root, fileCandidates, candidateIndex, unknownIds, unknownReasons, boundarySources, ts, 'virtual-module-boundary');
          diagnostics.push(diagnostic(
            'virtual-module-boundary',
            `module ${JSON.stringify(argument.text)} is resolved by an unsupported virtual module or loader`,
            relativePath(root, sourceFile.fileName),
            sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
          ));
        }
        else {
          markModuleSpecifier(
            argument,
            sourceFile,
            root,
            fileCandidates,
            usedIds,
            options,
            candidateIndex,
            unknownIds,
            unknownReasons,
            ts,
            edges,
            'runtime',
            'dynamic-import',
          );
        }
      } else if (argument) {
        markRuntimeBoundary(
          node,
          sourceFile,
          root,
          fileCandidates,
          candidateIndex,
          unknownIds,
          unknownReasons,
          boundarySources,
          ts,
          'dynamic-import-boundary',
        );
        diagnostics.push(diagnostic(
          'dynamic-import',
          'dynamic import path is unresolved',
          relativePath(root, sourceFile.fileName),
          sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
        ));
      }
      markDynamicImportBindings(
        node,
        sourceFile,
        root,
        checker,
        candidateIndex,
        fileIdsByPath,
        usedIds,
        ts,
        symbolCache,
      );
    } else if (node.kind === ts.SyntaxKind.CallExpression && isRequireCall(node, ts)) {
      const argument = node.arguments[0];
      if (argument && ts.isStringLiteralLike(argument)) {
        if (isVirtualOrLoaderSpecifier(argument.text)) {
          markRuntimeBoundary(node, sourceFile, root, fileCandidates, candidateIndex, unknownIds, unknownReasons, boundarySources, ts, 'virtual-module-boundary');
          diagnostics.push(diagnostic(
            'virtual-module-boundary',
            `module ${JSON.stringify(argument.text)} is resolved by an unsupported virtual module or loader`,
            relativePath(root, sourceFile.fileName),
            sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
          ));
        }
        else {
          markModuleSpecifier(
            argument,
            sourceFile,
            root,
            fileCandidates,
            usedIds,
            options,
            candidateIndex,
            unknownIds,
            unknownReasons,
            ts,
            edges,
            'runtime',
            'require',
          );
        }
      } else {
        markRuntimeBoundary(
          node,
          sourceFile,
          root,
          fileCandidates,
          candidateIndex,
          unknownIds,
          unknownReasons,
          boundarySources,
          ts,
          'dynamic-require-boundary',
        );
        diagnostics.push(diagnostic(
          'dynamic-require-boundary',
          'CommonJS require path is unresolved',
          relativePath(root, sourceFile.fileName),
          sourceFile.getLineAndCharacterOfPosition(safeStart(node)).line + 1,
        ));
      }
    }
    if (ts.isCallExpression(node) && !isImportCall(node, ts) && !isInCommonJsExport(node, ts)) {
      recordCallEdge(
        node,
        sourceFile,
        root,
        checker,
        candidateIndex,
        fileIdsByPath,
        usedIds,
        ts,
        symbolCache,
        edges,
      );
    } else if (ts.isNewExpression(node)) {
      recordDirectedEdge(
        node,
        'instantiates',
        sourceFile,
        root,
        checker,
        candidateIndex,
        fileIdsByPath,
        usedIds,
        ts,
        symbolCache,
        edges,
      );
    }
    ts.forEachChild(node, visit);
  }
  visit(sourceFile);
}

function recordCallEdge(call, sourceFile, root, checker, candidateIndex, fileIdsByPath, usedIds, ts, symbolCache, edges) {
  recordDirectedEdge(call, 'call', sourceFile, root, checker, candidateIndex, fileIdsByPath, usedIds, ts, symbolCache, edges);
}

function recordDirectedEdge(operation, kind, sourceFile, root, checker, candidateIndex, fileIdsByPath, usedIds, ts, symbolCache, edges) {
  const targetNode = callTargetNode(operation, ts);
  if (!targetNode) {
    return;
  }
  const symbol = resolveSymbol(checker, targetNode, ts);
  if (!symbol) {
    return;
  }
  const sourcePath = sourcePathForCandidate(sourceFile.fileName);
  const resolution = markSymbol(
    symbol,
    root,
    sourcePath,
    candidateIndex,
    fileIdsByPath,
    usedIds,
    ts,
    symbolCache,
  );
  if (!resolution.ids.length) {
    return;
  }
  const relativeSourcePath = relativePath(root, sourcePath);
  const source = moduleEdgeSource(operation, sourcePath, relativeSourcePath, candidateIndex, ts);
  if (!source) {
    return;
  }
  const start = safeStart(operation);
  const location = originalCallLocation(sourceFile, sourcePath, start);
  for (const target of resolution.ids) {
    edges.push({
      source,
      target,
      kind,
      path: relativeSourcePath,
      start,
      line: location.line,
      column: location.column,
      confidence: resolution.ids.length === 1 ? 'exact' : 'potential',
      mode: 'runtime',
      provenance: 'typescript',
    });
  }
}

function recordCommonJsExportEdges(assignment, values, sourceFile, root, checker, candidateIndex, fileIdsByPath, ts, symbolCache, edges) {
  if (!values.length) {
    return;
  }
  const sourcePath = sourcePathForCandidate(sourceFile.fileName);
  const relativeSourcePath = relativePath(root, sourcePath);
  const source = callerCandidateId(assignment, sourcePath, candidateIndex, ts)
    || `file::${relativeSourcePath}`;
  const start = safeStart(assignment);
  const location = originalCallLocation(sourceFile, sourcePath, start);
  for (const value of values) {
    const symbol = resolveSymbol(checker, value, ts);
    if (!symbol) {
      continue;
    }
    const resolution = markSymbol(
      symbol,
      root,
      sourcePath,
      candidateIndex,
      fileIdsByPath,
      new Set(),
      ts,
      symbolCache,
    );
    for (const target of resolution.ids) {
      edges.push({
        source,
        target,
        kind: 'commonjs-export',
        path: relativeSourcePath,
        start,
        line: location.line,
        column: location.column,
        confidence: resolution.ids.length === 1 ? 'exact' : 'potential',
        mode: 'runtime',
        provenance: 'typescript',
      });
    }
  }
}

function originalCallLocation(sourceFile, sourcePath, start) {
  const location = sourceFile.getLineAndCharacterOfPosition(start);
  const line = location.line + 1;
  const column = location.character + 1;
  if (!sourcePath.toLowerCase().endsWith('.vue')) {
    return { line, column };
  }
  return { line: null, column: null };
}

function callTargetNode(call, ts) {
  const expression = call.expression;
  if (ts.isIdentifier(expression) || ts.isPrivateIdentifier(expression)) {
    return expression;
  }
  if (ts.isPropertyAccessExpression(expression)) {
    return expression.name;
  }
  return expression;
}

function commonJsExportAssignment(node, ts) {
  if (!ts.isBinaryExpression(node)
    || node.operatorToken.kind !== ts.SyntaxKind.EqualsToken) {
    return undefined;
  }
  const target = commonJsExportTarget(node.left, ts);
  if (!target) {
    return undefined;
  }
  const values = collectCommonJsExportValues(node.right, ts);
  return {
    dynamic: target.dynamic || values.dynamic,
    values: target.dynamic || values.dynamic ? [] : values.values,
  };
}

function commonJsExportTarget(node, ts) {
  if (ts.isIdentifier(node) && node.text === 'exports') {
    return { dynamic: false };
  }
  if (isCommonJsExportBase(node, ts)) {
    return { dynamic: false };
  }
  if (ts.isPropertyAccessExpression(node)) {
    if (isCommonJsExportBase(node.expression, ts)) {
      return { dynamic: false };
    }
    return undefined;
  }
  if (ts.isElementAccessExpression(node) && isCommonJsExportBase(node.expression, ts)) {
    return {
      dynamic: !node.argumentExpression || !ts.isStringLiteralLike(node.argumentExpression),
    };
  }
  return undefined;
}

function isCommonJsExportBase(node, ts) {
  if (ts.isIdentifier(node) && node.text === 'exports') {
    return true;
  }
  if (ts.isPropertyAccessExpression(node)) {
    return ts.isIdentifier(node.expression)
      && node.expression.text === 'module'
      && node.name.text === 'exports';
  }
  if (ts.isElementAccessExpression(node)) {
    return ts.isIdentifier(node.expression)
      && node.expression.text === 'module'
      && node.argumentExpression
      && ts.isStringLiteralLike(node.argumentExpression)
      && node.argumentExpression.text === 'exports';
  }
  return false;
}

function collectCommonJsExportValues(node, ts) {
  if (ts.isParenthesizedExpression(node)
    || ts.isAsExpression(node)
    || ts.isTypeAssertionExpression(node)
    || (ts.isSatisfiesExpression && ts.isSatisfiesExpression(node))) {
    return collectCommonJsExportValues(node.expression, ts);
  }
  if (ts.isIdentifier(node)
    || ts.isPrivateIdentifier(node)
    || (ts.isPropertyAccessExpression(node) && !node.name.text.startsWith('#'))
    || (ts.isElementAccessExpression(node)
      && node.argumentExpression
      && ts.isStringLiteralLike(node.argumentExpression))) {
    return { dynamic: false, values: [node] };
  }
  if (ts.isObjectLiteralExpression(node)) {
    const values = [];
    let dynamic = false;
    for (const property of node.properties) {
      if (ts.isSpreadAssignment(property)) {
        const spread = collectCommonJsExportValues(property.expression, ts);
        dynamic ||= spread.dynamic;
        values.push(...spread.values);
        continue;
      }
      if (ts.isMethodDeclaration(property)
        || ts.isGetAccessorDeclaration(property)
        || ts.isSetAccessorDeclaration(property)) {
        if (property.name && property.name.kind !== ts.SyntaxKind.ComputedPropertyName) {
          values.push(property.name);
        } else {
          dynamic = true;
        }
        continue;
      }
      if (!ts.isPropertyAssignment(property) && !ts.isShorthandPropertyAssignment(property)) {
        continue;
      }
      if (property.name && property.name.kind === ts.SyntaxKind.ComputedPropertyName) {
        dynamic = true;
        continue;
      }
      if (ts.isShorthandPropertyAssignment(property)) {
        values.push(property.name);
        continue;
      }
      const value = collectCommonJsExportValues(property.initializer, ts);
      dynamic ||= value.dynamic;
      values.push(...value.values);
    }
    return { dynamic, values };
  }
  if (ts.isArrowFunction(node)
    || ts.isFunctionExpression(node)
    || ts.isClassExpression(node)
    || node.kind === ts.SyntaxKind.NullKeyword
    || node.kind === ts.SyntaxKind.TrueKeyword
    || node.kind === ts.SyntaxKind.FalseKeyword
    || ts.isStringLiteralLike(node)
    || ts.isNumericLiteral(node)) {
    return { dynamic: false, values: [] };
  }
  return { dynamic: true, values: [] };
}

function isInCommonJsExport(node, ts) {
  let current = node.parent;
  while (current) {
    if (ts.isBinaryExpression(current)
      && current.operatorToken.kind === ts.SyntaxKind.EqualsToken
      && commonJsExportTarget(current.left, ts)) {
      return true;
    }
    if (ts.isSourceFile(current)) {
      return false;
    }
    current = current.parent;
  }
  return false;
}

function callerCandidateId(call, sourcePath, candidateIndex, ts) {
  let current = call.parent;
  while (current && !ts.isSourceFile(current)) {
    let declaration;
    if (ts.isFunctionDeclaration(current)
      || ts.isClassDeclaration(current)
      || ts.isMethodDeclaration(current)
      || ts.isGetAccessorDeclaration(current)
      || ts.isSetAccessorDeclaration(current)
      || ts.isConstructorDeclaration(current)) {
      declaration = current;
    } else if (ts.isArrowFunction(current) || ts.isFunctionExpression(current)) {
      if (ts.isVariableDeclaration(current.parent)) {
        declaration = current.parent;
      } else {
        let container = current.parent;
        while (container
          && !ts.isSourceFile(container)
          && !ts.isFunctionLike(container)) {
          if (ts.isVariableDeclaration(container)) {
            declaration = container;
            break;
          }
          container = container.parent;
        }
      }
    }
    if (declaration) {
      const name = ts.isConstructorDeclaration(declaration)
        ? 'constructor'
        : declarationIdentifier(declaration, ts);
      if (name) {
        const ids = markCandidatesForDeclaration(
          candidateIndex,
          sourcePath,
          name,
          declaration,
          ts,
        );
        if (ids.length === 1) {
          return ids[0];
        }
      }
    }
    current = current.parent;
  }
  return undefined;
}

function uniqueEdges(edges) {
  const sorted = edges.sort((left, right) => (
    left.source.localeCompare(right.source)
    || left.target.localeCompare(right.target)
    || left.kind.localeCompare(right.kind)
    || left.path.localeCompare(right.path)
    || left.mode.localeCompare(right.mode)
    || left.provenance.localeCompare(right.provenance)
    || left.start - right.start
  ));
  return sorted.filter((edge, index) => index === 0
    || edge.source !== sorted[index - 1].source
    || edge.target !== sorted[index - 1].target
    || edge.kind !== sorted[index - 1].kind
    || edge.path !== sorted[index - 1].path
    || edge.mode !== sorted[index - 1].mode
    || edge.provenance !== sorted[index - 1].provenance
    || edge.start !== sorted[index - 1].start);
}

function markDynamicImportBindings(importCall, sourceFile, root, checker, candidateIndex, fileIdsByPath, usedIds, ts, symbolCache) {
  let expression = importCall;
  while (expression.parent
    && (ts.isAwaitExpression(expression.parent) || ts.isParenthesizedExpression(expression.parent))) {
    expression = expression.parent;
  }
  const declaration = expression.parent;
  if (!declaration || !ts.isVariableDeclaration(declaration) || !ts.isObjectBindingPattern(declaration.name)) {
    return;
  }
  let moduleType = checker.getTypeAtLocation(importCall);
  if (checker.getAwaitedType) {
    moduleType = checker.getAwaitedType(moduleType) || moduleType;
  }
  for (const element of declaration.name.elements) {
    const exportName = bindingExportName(element, ts);
    if (!exportName) {
      continue;
    }
    const symbol = checker.getPropertyOfType(moduleType, exportName);
    if (!symbol) {
      continue;
    }
    markSymbol(
      symbol,
      root,
      sourcePathForCandidate(sourceFile.fileName),
      candidateIndex,
      fileIdsByPath,
      usedIds,
      ts,
      symbolCache,
    );
  }
}

function bindingExportName(element, ts) {
  const name = element.propertyName || element.name;
  if (ts.isIdentifier(name) || ts.isStringLiteralLike(name) || ts.isNumericLiteral(name)) {
    return name.text;
  }
  return undefined;
}

function markSymbol(symbol, root, referencePath, candidateIndex, fileIdsByPath, usedIds, ts, symbolCache, seen = new Set()) {
  if (symbolCache.has(symbol)) {
    const cached = symbolCache.get(symbol);
    for (const id of cached.ids) {
      usedIds.add(id);
    }
    for (const file of cached.files) {
      if (file.path !== referencePath) {
        usedIds.add(file.id);
      }
    }
    return cached;
  }
  if (seen.has(symbol)) {
    return { matched: false, implementation: false, ids: [] };
  }
  seen.add(symbol);
  const declarations = symbol.declarations || (symbol.valueDeclaration ? [symbol.valueDeclaration] : []);
  let matched = false;
  let implementation = false;
  const resolvedIds = [];
  const resolvedFiles = [];
  for (const declaration of declarations) {
    const declarationFile = declaration.getSourceFile();
    const declarationPath = sourcePathForCandidate(declarationFile.fileName);
    if (!isProjectSource(root, declarationPath)) {
      continue;
    }
    const fileId = fileIdsByPath.get(declarationPath);
    if (fileId) {
      resolvedFiles.push({ path: declarationPath, id: fileId });
      if (declarationPath !== referencePath) {
        usedIds.add(fileId);
      }
    }
    const declarationName = declarationIdentifier(declaration, ts) || symbol.name;
    const ids = markCandidatesForDeclaration(
      candidateIndex,
      declarationPath,
      declarationName,
      declaration,
      ts,
    );
    for (const id of ids) {
      usedIds.add(id);
      resolvedIds.push(id);
    }
    matched ||= ids.length > 0;
    implementation ||= declaration.kind === ts.SyntaxKind.MethodDeclaration;
  }
  const result = {
    matched,
    implementation,
    ids: [...new Set(resolvedIds)],
    files: [...new Map(resolvedFiles.map(file => [file.id, file])).values()],
  };
  symbolCache.set(symbol, result);
  return result;
}

function markCandidatesForDeclaration(candidateIndex, declarationPath, declarationName, declaration, ts) {
  const normalizedPath = normalizePath(declarationPath);
  const samePath = candidateIndex.byPath.get(normalizedPath) || [];
  if (!samePath.length) {
    return [];
  }
  const sameName = candidateIndex.byPathAndName.get(candidateKey(normalizedPath, declarationName)) || [];
  const isVue = declarationPath.toLowerCase().endsWith('.vue');
  const isDefaultVueExport = isVue && declaration.kind === ts.SyntaxKind.ExportAssignment;
  const matches = isDefaultVueExport
    ? samePath.filter(candidate => candidate.name === 'default' || candidate.name === path.basename(declarationPath))
    : sameName;
  if (!matches.length) {
    return [];
  }
  const exact = matches.filter(candidate => startMatches(candidate.start, declaration));
  const topLevel = matches.filter(candidate => candidate.topLevel);
  const selected = exact.length === 1
    ? exact
    : isVue && topLevel.length === 1
      ? topLevel
    : isVue && matches.length === 1
      ? matches
      : !hasStart(matches) && matches.length === 1
        ? matches
        : [];
  return selected.map(candidate => candidate.id);
}

function indexCandidates(candidates) {
  const byPath = new Map();
  const byPathAndName = new Map();
  for (const candidate of candidates) {
    const pathCandidates = byPath.get(candidate.path) || [];
    pathCandidates.push(candidate);
    byPath.set(candidate.path, pathCandidates);
    const key = candidateKey(candidate.path, candidate.name);
    const namedCandidates = byPathAndName.get(key) || [];
    namedCandidates.push(candidate);
    byPathAndName.set(key, namedCandidates);
  }
  return { byPath, byPathAndName };
}

function candidateKey(candidatePath, name) {
  return `${candidatePath}\0${name}`;
}

function markModuleSpecifier(specifier, sourceFile, root, fileCandidates, usedIds, options, candidateIndex, unknownIds, unknownReasons, ts, edges, mode, kind) {
  if (!specifier || !specifier.text) {
    return;
  }
  const sourcePath = sourcePathForCandidate(sourceFile.fileName);
  const resolved = resolveLocalModule(specifier.text, sourcePath, root, options);
  if (!resolved) {
    return;
  }
  const normalized = normalizePath(resolved);
  if (mode === 'type') {
    return;
  }
  const relativeSourcePath = relativePath(root, sourcePath);
  const start = safeStart(specifier);
  const location = originalCallLocation(sourceFile, sourcePath, start);
  const source = moduleEdgeSource(specifier, sourcePath, relativeSourcePath, candidateIndex, ts);
  for (const candidate of fileCandidates) {
    if (candidate.path === normalized) {
      if (!source) {
        markUnknown(unknownIds, unknownReasons, candidate.id, 'runtime-dispatch-ambiguous');
        for (const target of candidateIndex.byPath.get(normalized) || []) {
          markUnknown(unknownIds, unknownReasons, target.id, 'runtime-dispatch-ambiguous');
        }
        continue;
      }
      usedIds.add(candidate.id);
      edges.push({
        source,
        target: candidate.id,
        kind,
        path: relativeSourcePath,
        start,
        line: location.line,
        column: location.column,
        confidence: 'exact',
        mode,
        provenance: 'typescript',
      });
    }
  }
}

function moduleEdgeSource(node, sourcePath, relativeSourcePath, candidateIndex, ts) {
  let current = node.parent;
  while (current && !ts.isSourceFile(current)) {
    if (ts.isPropertyDeclaration(current) && !current.modifiers?.some(modifier => modifier.kind === ts.SyntaxKind.StaticKeyword)) {
      return undefined;
    }
    current = current.parent;
  }
  const candidate = callerCandidateId(node, sourcePath, candidateIndex, ts);
  if (candidate) {
    return candidate;
  }
  current = node.parent;
  while (current && !ts.isSourceFile(current)) {
    if (ts.isFunctionLike(current)
      || ts.isClassLike(current)
      || (ts.isClassStaticBlockDeclaration && ts.isClassStaticBlockDeclaration(current))) {
      return undefined;
    }
    current = current.parent;
  }
  return `file::${relativeSourcePath}`;
}

function importDeclarationMode(node, ts) {
  const clause = node.importClause;
  if (!clause) {
    return 'runtime';
  }
  if (clause.isTypeOnly) {
    return 'type';
  }
  const bindings = clause.namedBindings;
  return !clause.name
    && bindings
    && ts.isNamedImports(bindings)
    && bindings.elements.length > 0
    && bindings.elements.every(element => element.isTypeOnly)
    ? 'type'
    : 'runtime';
}

function resolveLocalModule(specifier, containingFile, root, options) {
  const bases = [];
  if (specifier.startsWith('.') || specifier.startsWith('/')) {
    bases.push(specifier.startsWith('/') ? path.resolve(root, `.${specifier}`) : path.resolve(path.dirname(containingFile), specifier));
  } else {
    for (const [pattern, replacements] of Object.entries(options.paths || {})) {
      const wildcard = pattern.indexOf('*');
      if (wildcard < 0 && pattern !== specifier) {
        continue;
      }
      if (wildcard >= 0) {
        const prefix = pattern.slice(0, wildcard);
        const suffix = pattern.slice(wildcard + 1);
        if (!specifier.startsWith(prefix) || !specifier.endsWith(suffix)) {
          continue;
        }
        const value = specifier.slice(prefix.length, specifier.length - suffix.length || undefined);
        for (const replacement of replacements) {
          bases.push(path.resolve(options.pathsBasePath || root, replacement.replace('*', value)));
        }
      } else {
        for (const replacement of replacements) {
          bases.push(path.resolve(options.pathsBasePath || root, replacement));
        }
      }
    }
  }
  for (const base of bases) {
    const file = findModuleFile(base);
    if (file) {
      return file;
    }
  }
  return undefined;
}

function findModuleFile(base) {
  const candidates = [base, ...MODULE_EXTENSIONS.map(extension => `${base}${extension}`), ...MODULE_EXTENSIONS.map(extension => path.join(base, `index${extension}`))];
  return candidates.find(candidate => isFile(candidate));
}

function isReferenceIdentifier(node, ts) {
  if (isDeclarationName(node, ts)
    || isInImportOrExport(node, ts)
    || isInCommonJsExport(node, ts)
    || isPropertyName(node, ts)) {
    return false;
  }
  if (node.text.startsWith('__VLS_')) {
    return false;
  }
  return true;
}

function isTypeReference(node, ts) {
  let current = node.parent;
  while (current) {
    if (ts.isTypeNode(current)) {
      return true;
    }
    if (ts.isSourceFile(current)) {
      return false;
    }
    current = current.parent;
  }
  return false;
}

function isDeclarationName(node, ts) {
  const parent = node.parent;
  if (!parent) {
    return false;
  }
  if (ts.isVariableDeclaration(parent) || ts.isFunctionDeclaration(parent) || ts.isClassDeclaration(parent) || ts.isInterfaceDeclaration(parent) || ts.isTypeAliasDeclaration(parent) || ts.isEnumDeclaration(parent) || ts.isModuleDeclaration(parent) || ts.isParameter(parent) || ts.isTypeParameterDeclaration(parent) || ts.isBindingElement(parent) || ts.isEnumMember(parent)) {
    return parent.name === node;
  }
  if (ts.isMethodDeclaration(parent) || ts.isMethodSignature(parent) || ts.isPropertyDeclaration(parent) || ts.isPropertySignature(parent) || ts.isGetAccessorDeclaration(parent) || ts.isSetAccessorDeclaration(parent)) {
    return parent.name === node;
  }
  return false;
}

function isInImportOrExport(node, ts) {
  let current = node.parent;
  while (current) {
    if (ts.isImportDeclaration(current) || ts.isImportClause(current) || ts.isImportSpecifier(current) || ts.isNamespaceImport(current) || ts.isImportEqualsDeclaration(current) || ts.isExportDeclaration(current) || ts.isExportSpecifier(current) || ts.isExportAssignment(current)) {
      return true;
    }
    if (ts.isSourceFile(current)) {
      return false;
    }
    current = current.parent;
  }
  return false;
}

function isPropertyName(node, ts) {
  const parent = node.parent;
  if (!parent) {
    return false;
  }
  if (ts.isPropertyAssignment(parent) || ts.isMethodDeclaration(parent) || ts.isMethodSignature(parent) || ts.isPropertyDeclaration(parent) || ts.isPropertySignature(parent) || ts.isGetAccessorDeclaration(parent) || ts.isSetAccessorDeclaration(parent)) {
    return parent.name === node && !parent.computed;
  }
  if (ts.isJsxAttribute(parent) && parent.name === node) {
    return true;
  }
  if (ts.isJsxNamespacedName(parent)) {
    return true;
  }
  return false;
}

function declarationIdentifier(declaration, ts) {
  const name = declaration.name;
  return name ? semanticIdentifierName(name, ts) : undefined;
}

function startMatches(start, declaration) {
  if (start === undefined) {
    return false;
  }
  const sourceFile = declaration.getSourceFile();
  const offset = safeStart(declaration);
  if (typeof start === 'number') {
    const line = sourceFile.getLineAndCharacterOfPosition(Math.max(0, offset)).line + 1;
    return start === offset || start === line || start === line - 1;
  }
  if (!start || typeof start !== 'object') {
    return false;
  }
  if (typeof start.offset === 'number' || typeof start.position === 'number') {
    return start.offset === offset || start.position === offset;
  }
  const location = sourceFile.getLineAndCharacterOfPosition(Math.max(0, offset));
  const line = Number(start.line);
  const column = start.column ?? start.character;
  return Number.isFinite(line) && (line === location.line + 1 || line === location.line) && (column === undefined || column === location.character || column === location.character + 1);
}

function safeStart(node) {
  try {
    return node.name && node.name.getStart ? node.name.getStart() : node.getStart();
  } catch {
    return Math.max(0, node.pos || 0);
  }
}

function hasStart(candidates) {
  return candidates.some(candidate => candidate.start !== undefined);
}

function normalizeCandidates(root, rawCandidates) {
  return Array.isArray(rawCandidates)
    ? rawCandidates
      .filter(candidate => candidate && candidate.id !== undefined && candidate.path && candidate.name)
      .filter(candidate => !isDeclarationFile(candidate.path))
      .map(candidate => ({
        id: String(candidate.id),
        path: normalizePath(resolveFile(root, candidate.path)),
        name: String(candidate.name),
        kind: candidate.kind ?? candidate.nodeKind,
        start: candidate.start,
        topLevel: Boolean(candidate.topLevel),
      }))
    : [];
}

function isLikelyFileCandidate(candidate) {
  return candidate.kind === 'file';
}

function isDeclarationFile(file) {
  return /\.d\.(?:[cm]?[jt]sx?)$/i.test(String(file));
}

function sourcePathForCandidate(file) {
  const normalized = normalizePath(file);
  return normalized.replace(/\.vue\.(?:[cm]?[jt]sx?)$/i, '.vue');
}

function loadCompilerSfc(root) {
  const rootRequire = createRequire(path.join(root, 'package.json'));
  for (const packageName of ['@vue/compiler-sfc', 'vue-template-compiler', 'vue/compiler-sfc']) {
    try {
      return rootRequire(packageName);
    } catch {
      // vue-tsc dependency fallback below.
    }
  }
  const vueTsc = rootRequire.resolve('vue-tsc');
  return createRequire(vueTsc)('@vue/compiler-sfc');
}

function loadVueDependencies(root) {
  const rootRequire = createRequire(path.join(root, 'package.json'));
  let typescript;
  try {
    typescript = rootRequire('typescript');
  } catch {
    typescript = undefined;
  }
  let vueTsc;
  try {
    vueTsc = rootRequire.resolve('vue-tsc');
  } catch {
    return { typescript };
  }
  const vueRequire = createRequire(vueTsc);
  typescript ||= (() => {
    try {
      return vueRequire('typescript');
    } catch {
      return undefined;
    }
  })();
  if (!typescript) {
    return {};
  }
  try {
    return {
      typescript,
      languageCore: vueRequire('@vue/language-core'),
      volarTypescript: vueRequire('@volar/typescript'),
      volarLanguageCore: vueRequire('@volar/language-core'),
    };
  } catch {
    return { typescript };
  }
}

function resolveRoot(rawRoot) {
  if (typeof rawRoot !== 'string' || !rawRoot) {
    throw new Error('root is required');
  }
  const root = path.resolve(rawRoot);
  if (!isDirectory(root)) {
    throw new Error(`project root is not a directory: ${root}`);
  }
  return root;
}

function resolveFile(root, rawFile) {
  const file = String(rawFile);
  return path.resolve(root, file);
}

function relativePath(root, file) {
  return normalizePath(path.relative(root, file));
}

function normalizePath(file) {
  return path.normalize(file).replaceAll(path.sep, '/');
}

function isProjectSource(root, file) {
  const normalizedRoot = normalizePath(root);
  const normalizedFile = normalizePath(file);
  return normalizedFile === normalizedRoot || normalizedFile.startsWith(`${normalizedRoot}/`);
}

function isTestSource(file) {
  const normalized = normalizePath(file);
  const base = path.posix.basename(normalized);
  return normalized.split('/').some(part => ['__tests__', 'test', 'tests', 'e2e', 'cypress'].includes(part))
    || base.includes('.test.')
    || base.includes('.spec.')
    || base.startsWith('test_')
    || /_test\.(?:[cm]?[jt]sx?)$/.test(base);
}

function uniquePaths(files) {
  return [...new Set(files.map(file => normalizePath(file)))];
}

function strings(value) {
  return Array.isArray(value) ? value.filter(item => typeof item === 'string') : [];
}

function isFile(file) {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

function isDirectory(directory) {
  try {
    return fs.statSync(directory).isDirectory();
  } catch {
    return false;
  }
}

function isImportCall(node, ts) {
  return node.expression && node.expression.kind === ts.SyntaxKind.ImportKeyword;
}

function isRequireCall(node, ts) {
  return ts.isIdentifier(node.expression) && node.expression.text === 'require';
}

function isVirtualOrLoaderSpecifier(specifier) {
  return typeof specifier === 'string'
    && (specifier.startsWith('virtual:')
      || specifier.startsWith('\0')
      || specifier.includes('?'));
}

function isImportMetaGlob(node, ts) {
  if (!ts.isPropertyAccessExpression(node.expression) || node.expression.name.text !== 'glob') {
    return false;
  }
  const receiver = node.expression.expression;
  return ts.isMetaProperty(receiver)
    && receiver.keywordToken === ts.SyntaxKind.ImportKeyword
    && receiver.name.text === 'meta';
}

function markGlobFiles(pattern, containingFile, root, options, fileCandidates, usedIds, candidateIndex, unknownIds, edges, sourceFile, globCall, ts) {
  const patterns = resolveGlobPatterns(pattern, containingFile, root, options);
  const sourcePath = sourcePathForCandidate(sourceFile.fileName);
  const relativeSourcePath = relativePath(root, sourcePath);
  const start = safeStart(globCall);
  const location = originalCallLocation(sourceFile, sourcePath, start);
  const source = moduleEdgeSource(globCall, sourcePath, relativeSourcePath, candidateIndex, ts);
  if (!source) {
    return;
  }
  for (const candidate of fileCandidates) {
    if (patterns.some(value => globMatch(value, candidate.path))) {
      usedIds.add(candidate.id);
      edges.push({
        source,
        target: candidate.id,
        kind: 'dynamic-import',
        path: relativeSourcePath,
        start,
        line: location.line,
        column: location.column,
        confidence: 'potential',
        mode: 'runtime',
        provenance: 'typescript',
      });
    }
  }
}

function markUnknown(unknownIds, unknownReasons, id, reason) {
  unknownIds.add(id);
  if (!unknownReasons.has(id)) {
    unknownReasons.set(id, reason);
  }
}

function markRuntimeBoundary(node, sourceFile, root, fileCandidates, candidateIndex, unknownIds, unknownReasons, boundarySources, ts, reason) {
  const sourcePath = sourcePathForCandidate(sourceFile.fileName);
  const relativeSourcePath = relativePath(root, sourcePath);
  const source = moduleEdgeSource(node, sourcePath, relativeSourcePath, candidateIndex, ts);
  if (!source) {
    return;
  }
  boundarySources.push({ source, reason });
}

function resolveGlobPatterns(specifier, containingFile, root, options) {
  const bases = [];
  if (specifier.startsWith('.') || specifier.startsWith('/')) {
    bases.push(specifier.startsWith('/')
      ? path.resolve(root, `.${specifier}`)
      : path.resolve(path.dirname(containingFile), specifier));
  } else {
    for (const [pattern, replacements] of Object.entries(options.paths || {})) {
      const wildcard = pattern.indexOf('*');
      if (wildcard < 0 && pattern !== specifier) {
        continue;
      }
      const prefix = wildcard < 0 ? pattern : pattern.slice(0, wildcard);
      const suffix = wildcard < 0 ? '' : pattern.slice(wildcard + 1);
      if (!specifier.startsWith(prefix) || !specifier.endsWith(suffix)) {
        continue;
      }
      const value = specifier.slice(prefix.length, specifier.length - suffix.length || undefined);
      for (const replacement of replacements) {
        bases.push(path.resolve(options.pathsBasePath || root, replacement.replace('*', value)));
      }
    }
  }
  return bases.map(normalizePath);
}

function globMatch(pattern, candidate) {
  let expression = '^';
  const normalized = normalizePath(pattern);
  for (let index = 0; index < normalized.length; index += 1) {
    const character = normalized[index];
    if (character === '*') {
      if (normalized[index + 1] === '*') {
        index += 1;
        if (normalized[index + 1] === '/') {
          index += 1;
          expression += '(?:.*/)?';
        } else {
          expression += '.*';
        }
      } else {
        expression += '[^/]*';
      }
    } else if (character === '?') {
      expression += '[^/]';
    } else {
      expression += character.replace(/[|\\{}()[\]^$+?.]/g, '\\$&');
    }
  }
  return new RegExp(`${expression}$`).test(normalizePath(candidate));
}

function formatSfcError(error) {
  return typeof error === 'string' ? error : error && error.message ? error.message : String(error);
}

function formatTsDiagnostic(diagnostic, ts) {
  return ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
}

function errorMessage(error) {
  return error && error.message ? error.message : String(error);
}

function diagnostic(code, message, filePath, line) {
  const result = { code, message };
  if (filePath) {
    result.path = normalizePath(filePath);
  }
  if (Number.isInteger(line) && line > 0) {
    result.line = line;
  }
  return result;
}

main();
process.stdout.write(serializedOutput || '{}\n', () => process.exit(0));
