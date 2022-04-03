import * as vscode from 'vscode';

export function activate(context: vscode.ExtensionContext) {

	let disposable = vscode.commands.registerCommand('n2.helloWorld', () => {
		vscode.window.showInformationMessage('Hello World from n2!');
	});

	context.subscriptions.push(disposable);
}

export function deactivate() {}
