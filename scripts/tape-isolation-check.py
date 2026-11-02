#!/usr/bin/env python3
"""Fail closed before #240 selectors until their execution binding is qualified.

The account-inventory incident showed that a private HOME alone does not prove
subprocess isolation. No selector may cross that boundary without a verified
source-bound wrapper and a pre-effect destination proof. This guard deliberately
refuses; it does not stand in for that missing proof or run the subject command.
"""
import sys

sys.exit("oxide#240 selector refused: source-bound pre-effect isolation is unproven")
