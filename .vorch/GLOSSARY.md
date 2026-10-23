# Glossary

Shared vEdit terms that differ from a coding agent's own tools and environment.
Mechanics and contracts live in the linked domain maps.

## Document

The shared editable text and file identity in one editor window, including an
unnamed draft. Several panes can show one document (`documents.md`, `projects.md`).

## Pane

One editor region with its own tabs, active document and navigation state.
It is a view of documents, not a second copy of their text (`projects.md`).

## Project

An opened folder and its associated editor context, such as files, language
services and running tasks. It is not the coding agent's current directory or a
requirement that the folder be a Git repository (`projects.md`).

## Other files

The editor workspace for loose files and unnamed documents outside an opened
folder project. It has editor layouts but no project folder services (`projects.md`).

## Workspace

The editor context owning documents and layout: a folder project or Other files.
CLI project operations resolve a folder workspace; they do not automatically open
or switch the user's visible project (`projects.md`, `control.md`).

## Session

The editor window's persisted projects, documents, layouts and preferences.
It is not a conversation with a coding agent (`application.md`, `platform.md`).

## Window

An independent running editor instance with its own session and document IDs.
A CLI window target identifies that instance, not a pane (`platform.md`, `control.md`).

## Task

A finite project command such as build, test or run-file, with collected output.
It is distinct from a chat task and from a long-running project application
(`execution.md`).

## Launch

A named project application configuration and its running process tree, selected
through Play or the CLI. It can run alongside the project's task (`execution.md`).

## Agent mark

A user-authored selection or diff comment collected for an agent, with captured
text and optional note. It is distinct from agent-authored review annotations
(`reviews.md`).

## Change review

A read-only, multi-file comparison of Git states with seen markers and user
comments. A CLI review manifest instead supplies annotations on open documents
(`reviews.md`, `control.md`).

## Recovery baseline

The saved comparison state retained alongside an unsaved draft for external-change
checks. Discovering newer disk content does not authorize replacing this baseline
or overwriting that content (`documents.md`).
