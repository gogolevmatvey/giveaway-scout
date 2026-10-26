import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { desktop, repository, run, windowsToolchain } from './windows-ui-toolchain.mjs';

const {environment} = windowsToolchain();
if (process.argv.slice(2).some(arg => arg !== '--no-build')) throw new Error('Usage: pnpm test:ui:native [--no-build]');
if (!process.argv.includes('--no-build')) run(process.execPath,['scripts/build-native-ui.mjs'],desktop,environment);
const project = path.join(repository,'tests/GiveawayScout.WindowsUi/GiveawayScout.WindowsUi.csproj');
const dotnet = path.join(process.env.ProgramFiles,'dotnet/dotnet.exe');
run(dotnet,['restore',project,'--locked-mode','--nologo'],repository,environment);
run(dotnet,['build',project,'--no-restore','--nologo','-warnaserror'],repository,environment);
const report = path.join(repository,'artifacts/ui-tests',new Date().toISOString().replace(/[:.]/g,'-')+'-'+randomUUID());
fs.mkdirSync(report,{recursive:true});
console.log('Native UI reports:',report);
run(process.execPath,[
  '--test','--test-concurrency=1',
  '--test-reporter=spec','--test-reporter-destination=stdout',
  '--test-reporter=junit',`--test-reporter-destination=${path.join(report,'junit.xml')}`,
  'tests/ui/native.wdio.mjs',
],desktop,{...environment,GIVEAWAY_SCOUT_UI_REPORT:report});
