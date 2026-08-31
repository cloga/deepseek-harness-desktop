import { useQuery } from '@tanstack/react-query'
import { invoke } from '@tauri-apps/api/core'

export interface AppConfig {
  port: number
  auto_start: boolean
  cli_link_enabled: boolean
  zoom_factor: number
  close_action: string
}

/// 共享的 app 配置查询：config-close-action 与 config-debug 共用同一份
/// queryKey/queryFn 定义，避免两处手写 useQuery 漂移（如一方加了 staleTime）。
export function useAppConfig() {
  return useQuery({
    queryKey: ['config'],
    queryFn: () => invoke<AppConfig>('get_app_config'),
  })
}
