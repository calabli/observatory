import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source=await readFile(new URL('../src/usage/appRanking.ts',import.meta.url),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const {rankApps}=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const summary=(cost,tokens,requests,input,read,write)=>({totalCost:cost,realTotalTokens:tokens,totalRequests:requests,totalInputTokens:input,totalCacheReadTokens:read,totalCacheCreationTokens:write});
const apps=[{appType:'a',summary:summary('1',900,2,100,0,0)},{appType:'b',summary:summary('5',100,8,10,90,0)},{appType:'c',summary:summary('0',0,0,0,0,0)}];
test('ranking follows selected metric and excludes absent hit-rate denominators',()=>{
 assert.equal(rankApps(apps,'tokens','')[0].appType,'a');
 assert.equal(rankApps(apps,'cost','')[0].appType,'b');
 assert.equal(rankApps(apps,'requests','')[0].appType,'b');
 const rates=rankApps(apps,'hitRate','');assert.equal(rates[0].appType,'b');assert.equal(rates[0].value,0.9);assert.equal(rates[2].value,null);
 assert.deepEqual(rankApps(apps,'cost','a').map(x=>x.appType),['a']);
});
