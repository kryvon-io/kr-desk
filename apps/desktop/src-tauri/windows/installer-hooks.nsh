; Ganchos do instalador NSIS (Tauri): serviço do KR Desk.
;
; O serviço roda como SYSTEM a partir de $INSTDIR, por isso o instalador é
; "perMachine" (Arquivos de Programas, onde só administradores escrevem).

!macro NSIS_HOOK_PREINSTALL
  ; Atualização: para o serviço (e o agente) antes de trocar o executável.
  ; Usa sc.exe porque a versão anterior pode não ter --stop-service.
  nsExec::Exec 'sc.exe stop KRDeskService'
  Pop $0
  Sleep 2000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --install-service' $0
  ${If} $0 != 0
    DetailPrint "Não foi possível instalar o serviço do KR Desk (código $0)."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-service' $0
!macroend
