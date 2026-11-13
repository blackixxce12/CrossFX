FxSound Port for Windows
========================

FxSound Port is the FxSound audio enhancer, ported to Rust: the equalizer, the effects and the
factory presets of FxSound for everything Windows plays. It is made by the FxSound Linux port
contributors and is not FxSound LLC's FxSound; the two can be installed side by side.

FxSound Port needs a virtual playback device, which it does not install: the FxSound Audio
Enhancer driver that comes with the original FxSound, or VB-CABLE.

What is in this folder

  fxsound.exe       the program
  presets\          the factory presets: Factsoft and BonusPresets (.fac), Input (.toml)
  LICENSE           FxSound Port's licence, the GNU Affero General Public License 3 or later
  COPYING.rnnoise   BSD-3-Clause, for the RNNoise noise suppression built into fxsound.exe
  OFL.noto          SIL Open Font License 1.1, for the Noto fonts built into fxsound.exe
  OFL.manrope       SIL Open Font License 1.1, for the Manrope font built into fxsound.exe
  README.txt        this file

The setup program and the .msi install FxSound Port into Program Files; it is removed from the
list of installed apps in Windows Settings. The .zip needs no installation: unpack it anywhere
and run fxsound.exe, keeping the presets folder beside it. Before deleting a copy from the .zip,
turn off "Launch on system startup" in its settings, or Windows keeps trying to start it at every
login.

Source code, issues and releases: https://github.com/blackixxce12/fxsound-linux


FxSound Port для Windows
========================

FxSound Port — это программа для улучшения звука FxSound, перенесённая на Rust: эквалайзер,
эффекты и заводские пресеты FxSound для всего, что воспроизводит Windows. Её делают участники
проекта FxSound для Linux, и это не FxSound от FxSound LLC; обе программы можно установить на
один компьютер.

Для работы FxSound Port нужно виртуальное устройство воспроизведения, которое программа не
устанавливает сама: драйвер FxSound Audio Enhancer из оригинального FxSound или VB-CABLE.

Содержимое этой папки

  fxsound.exe       сама программа
  presets\          заводские пресеты: Factsoft и BonusPresets (.fac), Input (.toml)
  LICENSE           лицензия FxSound Port — GNU Affero General Public License версии 3 или
                    более поздней
  COPYING.rnnoise   лицензия BSD-3-Clause для встроенного в fxsound.exe шумоподавления RNNoise
  OFL.noto          SIL Open Font License 1.1 для встроенных в fxsound.exe шрифтов Noto
  OFL.manrope       SIL Open Font License 1.1 для встроенного в fxsound.exe шрифта Manrope
  README.txt        этот файл

Программа установки и пакет .msi устанавливают FxSound Port в Program Files; удалить его можно
из списка установленных приложений в параметрах Windows. Архив .zip устанавливать не нужно:
распакуйте его в любую папку и запустите fxsound.exe, не отделяя от него папку presets. Прежде
чем удалить копию из архива, выключите в её настройках параметр «Автоматически запускать при
входе в систему», иначе Windows будет пытаться запускать её при каждом входе.

Исходный код, сообщения об ошибках и выпуски: https://github.com/blackixxce12/fxsound-linux
