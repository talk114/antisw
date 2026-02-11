import { request as invoke } from './request';

export interface TrackingMetadata {
    screen?: string;
    button?: string | null;
    [key: string]: any;
}

/**
 * Track an event
 * @param eventName - Name of the event (e.g., 'open_app', 'page_view', 'button_click')
 * @param userId - Optional user identifier
 * @param metadata - Additional event metadata
 */
export async function trackEvent(
    eventName: string,
    userId?: string,
    metadata: TrackingMetadata = {}
): Promise<void> {
    try {
        await invoke('track_event', {
            eventName,
            userId: userId || undefined,
            metadata,
        });
    } catch (error) {
        // Silently fail - tracking should not break the app
        console.warn('Failed to track event:', error);
    }
}

/**
 * Get device ID
 */
export async function getDeviceId(): Promise<string> {
    return await invoke<string>('get_tracking_device_id');
}

/**
 * Get machine name
 */
export async function getMachineName(): Promise<string> {
    return await invoke<string>('get_tracking_machine_name');
}

/**
 * Get OS
 */
export async function getOS(): Promise<string> {
    return await invoke<string>('get_tracking_os');
}

// Common event names
export const TrackingEvents = {
    APP_OPEN: 'open_app',
    APP_CLOSE: 'app_close',
    PAGE_VIEW: 'page_view',
    BUTTON_CLICK: 'button_click',
    FEATURE_USE: 'feature_use',
    ACCOUNT_ADD: 'account_add',
    ACCOUNT_SWITCH: 'account_switch',
    PROXY_START: 'proxy_start',
    PROXY_STOP: 'proxy_stop',
    UPDATE_CHECK: 'update_check',
} as const;
