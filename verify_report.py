#!/usr/bin/env python3
import os
import re
import sys

def verify():
    report_path = "docs/archive/codebase_analysis_report.md"
    if not os.path.exists(report_path):
        print(f"Error: {report_path} does not exist.")
        return False
        
    with open(report_path, "r", encoding="utf-8") as f:
        content = f.read()
        
    if len(content.strip()) < 500:
        print(f"Error: {report_path} is too short ({len(content)} characters).")
        return False
        
    # Check for required sections
    required_sections = [
        r"architectural\s+&\s+design",
        r"performance\s+&\s+optimization",
        r"code\s+quality\s+&\s+rust\s+idioms",
        r"verification\s+&\s+test\s+coverage",
        r"specification\s+alignment"
    ]
    
    content_lower = content.lower()
    for section_pattern in required_sections:
        if not re.search(section_pattern, content_lower):
            print(f"Error: Missing section matching pattern '{section_pattern}' in report.")
            return False
            
    # Check for code snippets
    code_blocks = re.findall(r"```(?:rust|diff)?\n(.*?)\n```", content, re.DOTALL)
    if len(code_blocks) < 5:
        print(f"Error: Expected at least 5 code/diff snippets in the report, found {len(code_blocks)}.")
        return False
        
    # Extract file paths from the text (looking for patterns like src/xxx.rs)
    file_paths = set(re.findall(r"\bsrc/[a-zA-Z0-9_\-/]+\.rs\b", content))
    if not file_paths:
        print("Warning: No file paths like 'src/*.rs' found in the report.")
    else:
        print(f"Checking referenced files: {file_paths}")
        for path in file_paths:
            if not os.path.exists(path):
                print(f"Error: Referenced file '{path}' does not exist in the repository.")
                return False

    print("Success: Report verification passed!")
    return True

if __name__ == "__main__":
    if verify():
        sys.exit(0)
    else:
        sys.exit(1)
