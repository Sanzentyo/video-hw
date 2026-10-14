//! Vulkan validation-message capture and probe instance construction.

use super::*;

fn clear_hevc_encode_validation_messages() {
    if let Some(messages) = HEVC_ENCODE_VALIDATION_MESSAGES.get() {
        if let Ok(mut guard) = messages.lock() {
            guard.clear();
        }
    } else {
        let _ = HEVC_ENCODE_VALIDATION_MESSAGES.set(Mutex::new(Vec::new()));
    }
}

fn push_hevc_encode_validation_message(message: impl Into<String>) {
    let messages = HEVC_ENCODE_VALIDATION_MESSAGES.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut guard) = messages.lock() {
        guard.push(message.into());
        if guard.len() > 16 {
            let drain = guard.len().saturating_sub(16);
            guard.drain(0..drain);
        }
    }
}

pub(super) fn append_hevc_encode_validation_messages(base: String) -> String {
    let Some(messages) = HEVC_ENCODE_VALIDATION_MESSAGES.get() else {
        return base;
    };
    let Ok(guard) = messages.lock() else {
        return base;
    };
    let collected = guard
        .iter()
        .rev()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if collected.is_empty() {
        base
    } else {
        format!("{base}; validation_messages={}", collected.join(" | "))
    }
}

// SAFETY: Vulkan invokes this function only while the registered debug messenger is alive. The
// callback validates both nullable pointers before dereferencing and does not retain borrowed data.
unsafe extern "system" fn hevc_encode_validation_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_types: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    let severity = if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        "error"
    } else if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        "warn"
    } else {
        "info"
    };
    let raw_message = if p_callback_data.is_null() {
        "no callback data".to_string()
    } else {
        // SAFETY: callback data pointer is provided by Vulkan for the callback duration.
        let callback_data = unsafe { &*p_callback_data };
        if callback_data.p_message.is_null() {
            "no validation message".to_string()
        } else {
            // SAFETY: Vulkan provides a valid null-terminated C string.
            unsafe { CStr::from_ptr(callback_data.p_message) }
                .to_string_lossy()
                .replace('\n', " ")
                .trim()
                .to_string()
        }
    };
    push_hevc_encode_validation_message(format!("[{severity}] {raw_message}"));
    vk::FALSE
}

pub(super) fn create_hevc_encode_probe_instance(
    entry: &ash::Entry,
) -> Result<(ash::Instance, Option<HevcEncodeValidationDebugState>), String> {
    clear_hevc_encode_validation_messages();

    // SAFETY: enumerating immutable instance extension metadata requires no additional invariants.
    let available_extensions = unsafe { entry.enumerate_instance_extension_properties(None) }
        .map_err(|err| format!("failed to enumerate Vulkan instance extensions: {err}"))?;
    let has_debug_utils = available_extensions.iter().any(|extension| {
        // SAFETY: Vulkan guarantees extension_name is null-terminated.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        name == vk::EXT_DEBUG_UTILS_NAME
    });

    let validation_layer_name = c"VK_LAYER_KHRONOS_validation";
    // SAFETY: enumerating immutable instance layer metadata requires no additional invariants.
    let available_layers = unsafe { entry.enumerate_instance_layer_properties() }
        .map_err(|err| format!("failed to enumerate Vulkan instance layers: {err}"))?;
    let has_validation_layer = available_layers.iter().any(|layer| {
        // SAFETY: Vulkan guarantees layer_name is null-terminated.
        let name = unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) };
        name == validation_layer_name
    });

    let mut enabled_extensions = Vec::new();
    if has_debug_utils {
        enabled_extensions.push(vk::EXT_DEBUG_UTILS_NAME.as_ptr());
    } else {
        push_hevc_encode_validation_message(
            "[info] VK_EXT_debug_utils is unavailable; validation callback disabled",
        );
    }
    let mut enabled_layers = Vec::new();
    if has_validation_layer {
        enabled_layers.push(validation_layer_name.as_ptr());
    } else {
        push_hevc_encode_validation_message(
            "[info] VK_LAYER_KHRONOS_validation is unavailable; driver validation messages may be missing",
        );
    }

    let create_info = vk::InstanceCreateInfo::default()
        .enabled_extension_names(&enabled_extensions)
        .enabled_layer_names(&enabled_layers);
    // SAFETY: instance create info references stack data valid for the call.
    let instance = unsafe { entry.create_instance(&create_info, None) }
        .map_err(|err| format!("failed to create Vulkan instance: {err}"))?;

    let debug_state = if has_debug_utils {
        let debug_utils = ash::ext::debug_utils::Instance::new(entry, &instance);
        let messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                    | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
            )
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE
                    | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
            )
            .pfn_user_callback(Some(hevc_encode_validation_callback));
        // SAFETY: debug utils extension and instance are valid in this scope.
        match unsafe { debug_utils.create_debug_utils_messenger(&messenger_info, None) } {
            Ok(messenger) => Some(HevcEncodeValidationDebugState {
                debug_utils,
                messenger,
            }),
            Err(err) => {
                push_hevc_encode_validation_message(format!(
                    "[warn] failed to create debug utils messenger: {err}"
                ));
                None
            }
        }
    } else {
        None
    };
    Ok((instance, debug_state))
}
