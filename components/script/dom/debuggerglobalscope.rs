/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use base::id::{Index, PipelineId, PipelineNamespaceId};
use constellation_traits::ScriptToConstellationChan;
use devtools_traits::{ScriptToDevtoolsControlMsg, WorkerId};
use dom_struct::dom_struct;
use embedder_traits::resources::{self, Resource};
use ipc_channel::ipc::IpcSender;
use js::jsval::UndefinedValue;
use js::rust::Runtime;
use js::rust::wrappers::JS_DefineDebuggerObject;
use net_traits::ResourceThreads;
use profile_traits::{mem, time};
use script_bindings::codegen::GenericBindings::DebuggerGlobalScopeBinding::{
    DebuggerGlobalScopeMethods, NotifyNewSource,
};
use script_bindings::realms::InRealm;
use script_bindings::reflector::DomObject;
use servo_url::{ImmutableOrigin, MutableOrigin, ServoUrl};

use crate::dom::bindings::codegen::Bindings::DebuggerGlobalScopeBinding;
use crate::dom::bindings::error::report_pending_exception;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::utils::define_all_exposed_interfaces;
use crate::dom::event::EventStatus;
use crate::dom::globalscope::GlobalScope;
use crate::dom::types::{DebuggerEvent, Event};
#[cfg(feature = "testbinding")]
#[cfg(feature = "webgpu")]
use crate::dom::webgpu::identityhub::IdentityHub;
use crate::realms::enter_realm;
use crate::script_module::ScriptFetchOptions;
use crate::script_runtime::{CanGc, JSContext};

#[derive(Clone, Debug, MallocSizeOf)]
pub(crate) enum ThreadInfo {
    ScriptThread,
    WorkerThread {
        worker_id: WorkerId,

        /// Pipeline id of the page that created this worker.
        ///
        /// Debugger globals in web worker threads should be created with this pipeline id, because web worker threads
        /// don’t have a pipeline namespace and the pipeline id only gets used for logging anyway.
        pipeline_id: PipelineId,
    },
}

impl ThreadInfo {
    fn pipeline_id(&self) -> PipelineId {
        match self {
            ThreadInfo::ScriptThread => PipelineId::new(),
            ThreadInfo::WorkerThread { pipeline_id, .. } => *pipeline_id,
        }
    }
    fn worker_id(&self) -> Option<WorkerId> {
        match self {
            ThreadInfo::ScriptThread => None,
            ThreadInfo::WorkerThread { worker_id, .. } => Some(*worker_id),
        }
    }
}

#[dom_struct]
/// Global scope for interacting with the devtools Debugger API.
///
/// <https://firefox-source-docs.mozilla.org/js/Debugger/>
pub(crate) struct DebuggerGlobalScope {
    global_scope: GlobalScope,
    #[no_trace]
    thread_info: ThreadInfo,
}

impl DebuggerGlobalScope {
    /// Create a new heap-allocated `DebuggerGlobalScope`.
    #[allow(unsafe_code, clippy::too_many_arguments)]
    pub(crate) fn new(
        runtime: &Runtime,
        thread_info: ThreadInfo,
        devtools_chan: Option<IpcSender<ScriptToDevtoolsControlMsg>>,
        mem_profiler_chan: mem::ProfilerChan,
        time_profiler_chan: time::ProfilerChan,
        script_to_constellation_chan: ScriptToConstellationChan,
        resource_threads: ResourceThreads,
        #[cfg(feature = "webgpu")] gpu_id_hub: std::sync::Arc<IdentityHub>,
        can_gc: CanGc,
    ) -> DomRoot<Self> {
        let global = Box::new(Self {
            global_scope: GlobalScope::new_inherited(
                thread_info.pipeline_id(),
                devtools_chan,
                mem_profiler_chan,
                time_profiler_chan,
                script_to_constellation_chan,
                resource_threads,
                MutableOrigin::new(ImmutableOrigin::new_opaque()),
                ServoUrl::parse_with_base(None, "about:internal/debugger")
                    .expect("Guaranteed by argument"),
                None,
                Default::default(),
                #[cfg(feature = "webgpu")]
                gpu_id_hub,
                None,
                false,
            ),
            thread_info,
        });
        let global = unsafe {
            DebuggerGlobalScopeBinding::Wrap::<crate::DomTypeHolder>(
                JSContext::from_ptr(runtime.cx()),
                global,
            )
        };

        let realm = enter_realm(&*global);
        define_all_exposed_interfaces(global.upcast(), InRealm::entered(&realm), can_gc);
        assert!(unsafe {
            // Invariants: `cx` must be a non-null, valid JSContext pointer,
            // and `obj` must be a handle to a JS global object.
            JS_DefineDebuggerObject(
                *Self::get_cx(),
                global.global_scope.reflector().get_jsobject(),
            )
        });

        global
    }

    /// Get the JS context.
    pub(crate) fn get_cx() -> JSContext {
        GlobalScope::get_cx()
    }

    pub(crate) fn as_global_scope(&self) -> &GlobalScope {
        self.upcast::<GlobalScope>()
    }

    fn evaluate_js(&self, script: &str, can_gc: CanGc) -> bool {
        rooted!(in (*Self::get_cx()) let mut rval = UndefinedValue());
        self.global_scope.evaluate_js_on_global_with_result(
            script,
            rval.handle_mut(),
            ScriptFetchOptions::default_classic_script(&self.global_scope),
            self.global_scope.api_base_url(),
            can_gc,
            None,
        )
    }

    pub(crate) fn execute(&self, can_gc: CanGc) {
        if !self.evaluate_js(&resources::read_string(Resource::DebuggerJS), can_gc) {
            let ar = enter_realm(self);
            report_pending_exception(Self::get_cx(), true, InRealm::Entered(&ar), can_gc);
        }
    }

    #[allow(unsafe_code)]
    pub(crate) fn fire_add_debuggee(
        &self,
        can_gc: CanGc,
        global: &GlobalScope,
        pipeline_id: PipelineId,
    ) {
        let pipeline_id =
            crate::dom::pipelineid::PipelineId::new(self.upcast(), pipeline_id, can_gc);
        let event = DomRoot::upcast::<Event>(DebuggerEvent::new(
            self.upcast(),
            global,
            &pipeline_id,
            self.thread_info.worker_id().map(|id| id.to_string().into()),
            can_gc,
        ));
        assert_eq!(
            DomRoot::upcast::<Event>(event).fire(self.upcast(), can_gc),
            EventStatus::NotCanceled,
            "Guaranteed by DebuggerEvent::new"
        );
    }
}

impl DebuggerGlobalScopeMethods<crate::DomTypeHolder> for DebuggerGlobalScope {
    // check-tidy: no specs after this line
    fn NotifyNewSource(&self, args: &NotifyNewSource) {
        info!(
            "NotifyNewSource: ({},{}) {} {} {}",
            args.pipelineId.namespaceId,
            args.pipelineId.index,
            args.spidermonkeyId,
            args.url,
            args.text
        );
        if let Some(devtools_chan) = self.as_global_scope().devtools_chan() {
            let pipeline_id = PipelineId {
                namespace_id: PipelineNamespaceId(args.pipelineId.namespaceId),
                index: Index::new(args.pipelineId.index)
                    .expect("`pipelineId.index` must not be zero"),
            };

            if let Some(introduction_type) = args.introductionType.as_ref() {
                // TODO: handle the other cases in
                // <https://searchfox.org/mozilla-central/rev/f6a806c38c459e0e0d797d264ca0e8ad46005105/devtools/server/actors/utils/source-url.js#34-39>
                // <https://searchfox.org/mozilla-central/rev/5446303cba9b19b9e88937be62936a96086dcf32/devtools/server/actors/source.js#65-98>

                // TODO: remove trailing details that may have been appended by SpiderMonkey (currently buggy)
                // <https://bugzilla.mozilla.org/show_bug.cgi?id=1982001>
                let url_original = args.url.str();

                let url_original = ServoUrl::parse(url_original).ok();
                let url_override = args
                    .urlOverride
                    .as_ref()
                    .map(|url| url.str())
                    // TODO: do we need to use the page url as base here, say if url_original fails to parse?
                    .and_then(|url| ServoUrl::parse_with_base(url_original.as_ref(), url).ok());

                // <https://searchfox.org/mozilla-central/rev/f6a806c38c459e0e0d797d264ca0e8ad46005105/devtools/server/actors/utils/source-url.js#21-33>
                if [
                    "injectedScript",
                    "eval",
                    "debugger eval",
                    "Function",
                    "javascriptURL",
                    "eventHandler",
                    "domTimer",
                ]
                .contains(&introduction_type.str()) &&
                    url_override.is_none()
                {
                    debug!(
                        "Not creating debuggee: `introductionType` is `{introduction_type}` but no valid url"
                    );
                    return;
                }

                // TODO: handle the other cases in
                // <https://searchfox.org/mozilla-central/rev/5446303cba9b19b9e88937be62936a96086dcf32/devtools/server/actors/source.js#126-133>
                let inline = introduction_type.str() == "inlineScript" && url_override.is_none();
                let Some(url) = url_override.or(url_original) else {
                    debug!("Not creating debuggee: no valid url");
                    return;
                };

                let worker_id = args.workerId.as_ref().map(|id| dbg!(id).parse().unwrap());

                let source_info = SourceInfo {
                    url,
                    introduction_type: introduction_type.str().to_owned(),
                    inline,
                    worker_id,
                    content: (!inline).then(|| args.text.to_string()),
                    content_type: None, // TODO
                    spidermonkey_id: args.spidermonkeyId,
                };
                devtools_chan
                    .send(ScriptToDevtoolsControlMsg::CreateSourceActor(
                        pipeline_id,
                        source_info,
                    ))
                    .expect("Failed to send to devtools server");
            } else {
                debug!("Not creating debuggee for script with no `introductionType`");
            }
        }
    }
}
