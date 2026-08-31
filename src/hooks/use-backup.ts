import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useEffect } from 'react'

/** Rust 侧 service::backup::BackupInfo 的序列化形态（camelCase） */
export interface BackupInfo {
  timestamp: string
  path: string
  size: number
  includeCredentials: boolean
}

export interface UseBackupsResult {
  backups: BackupInfo[]
  loading: boolean
  error: string
  createBackup: (includeCredentials: boolean) => Promise<BackupInfo>
  restoreBackup: (timestamp: string, asNew: boolean) => Promise<void>
  deleteBackup: (timestamp: string) => Promise<void>
  updateAutoBackupSettings: (settings: {
    autoBackupEnabled?: boolean
    autoBackupIntervalDays?: number
    autoBackupOnStartup?: boolean
    autoBackupOnChange?: boolean
    backupRetentionCount?: number
    backupIncludeCredentials?: boolean
  }) => Promise<void>
  busy: boolean
}

/**
 * 备份列表与操作（react-query）。
 *
 * 查询键 `['backups']`：备份设置存在桌面端 store，`update_app_config` 会触发
 * `setting_updated` 事件，这里监听该事件一并失效重拉，保证列表与后端一致。
 */
export function useBackups(): UseBackupsResult {
  const queryClient = useQueryClient()

  const { data, isLoading, error, refetch } = useQuery({
    queryKey: ['backups'],
    queryFn: () => invoke<BackupInfo[]>('list_backups'),
  })

  // 后端设置变更（配置变化触发备份等）后刷新备份列表
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    listen('setting_updated', () => {
      void queryClient.invalidateQueries({ queryKey: ['backups'] })
    })
      .then((fn) => {
        if (disposed)
          fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [queryClient])

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['backups'] })
  }

  const create = useMutation({
    mutationFn: (includeCredentials: boolean) =>
      invoke<BackupInfo>('backup_profile', { includeCredentials }),
    onSuccess: invalidate,
  })
  const restore = useMutation({
    mutationFn: ({ timestamp, asNew }: { timestamp: string, asNew: boolean }) =>
      invoke<void>('restore_profile', { timestamp, asNew }),
    onSuccess: invalidate,
  })
  const remove = useMutation({
    mutationFn: (timestamp: string) => invoke<void>('delete_backup', { timestamp }),
    onSuccess: invalidate,
  })
  const updateSettings = useMutation({
    mutationFn: (settings: {
      autoBackupEnabled?: boolean
      autoBackupIntervalDays?: number
      autoBackupOnStartup?: boolean
      autoBackupOnChange?: boolean
      backupRetentionCount?: number
      backupIncludeCredentials?: boolean
    }) => invoke<void>('update_app_config', settings),
    onSuccess: invalidate,
  })

  return {
    backups: data ?? [],
    loading: isLoading,
    error: error ? String(error) : '',
    createBackup: async (includeCredentials) => {
      const created = await create.mutateAsync(includeCredentials)
      await refetch()
      return created
    },
    restoreBackup: async (timestamp, asNew) => {
      await restore.mutateAsync({ timestamp, asNew })
      await refetch()
    },
    deleteBackup: async (timestamp) => {
      await remove.mutateAsync(timestamp)
      await refetch()
    },
    updateAutoBackupSettings: async (settings) => {
      await updateSettings.mutateAsync(settings)
      await refetch()
    },
    busy: create.isPending || restore.isPending || remove.isPending || updateSettings.isPending,
  }
}
