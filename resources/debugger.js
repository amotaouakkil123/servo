if ("dbg" in this) {
    throw new Error("Debugger script must not run more than once!");
}

const dbg = new Debugger;
const debuggeesToPipelineIds = new Map;
const debuggeesToWorkerIds = new Map;

dbg.onNewGlobalObject = function(global) {
};

dbg.onNewScript = function(script, /* undefined; seems to be `script.global` now */ global) {
    try {
        console.log("[debugger] onNewScript url=", script.url, "source id=", script.source.id, "introductionType=", script.source.introductionType, "displayURL=", script.source.displayURL);
        try {
            console.log("[debugger] source binary=", typeof script.source.binary);
        } catch (error) {
            // Do nothing; the source is not wasm
        }
        notifyNewSource({
            pipelineId: debuggeesToPipelineIds.get(script.global),
            workerId: debuggeesToWorkerIds.get(script.global),
            spidermonkeyId: script.source.id,
            url: script.source.url,
            urlOverride: script.source.displayURL,
            text: script.source.text,
            introductionType: script.source.introductionType ?? null,
        });
    } catch (error) {
        logError(error);
    }
};

addEventListener("addDebuggee", event => {
    const {global, pipelineId: {namespaceId, index}, workerId} = event;
    dbg.addDebuggee(global);
    const debuggerObject = dbg.addDebuggee(global);
    debuggeesToPipelineIds.set(debuggerObject, {
        namespaceId,
        index,
    });
    debuggeesToWorkerIds.set(debuggerObject, workerId);
});

function logError(error) {
    console.log(`[debugger] ERROR at ${error.fileName}:${error.lineNumber}:${error.columnNumber}: ${error.name}: ${error.message}`);
}
