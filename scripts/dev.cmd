@echo off
rem Lance dashu en local (Windows) avec les fichiers statiques relus sur le disque.
rem Prerequis : un fichier .env a la racine et un `cargo build` deja fait.
cd /d "%~dp0.."
set DASHU_STATIC_DIR=static
set DASHU_ALLOW_LOCAL_ICAL=1
target\debug\dashu.exe
