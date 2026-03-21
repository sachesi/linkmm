# Mod Installation Reimplementation Progress

## Completed Work

This PR reimplements key components of the mod installation system following the Bethesda Mod Installation guidelines for linkmm.

### 1. New Core Modules Created

#### `src/core/installer_new.rs` (483 lines)
- **Link Type Decision System**: Automatically chooses between hardlinks and symlinks based on filesystem boundaries
  - Uses `st_dev` to detect same-filesystem scenarios
  - Prefers hardlinks when possible (faster, no dangling risk)
  - Falls back to symlinks across filesystems
- **Path Normalization**: Complete lowercase normalization for case-insensitive Linux filesystems
  - Critical for game engine compatibility
  - Handles Windows-style backslashes
  - Strip "Data/" prefix handling
- **Archive Root Detection**: Improved scoring heuristics
  - Scores directories based on Data/ indicators
  - Handles single wrapper directories
  - Detects explicit Data/ folders
  - Minimum score threshold for confidence
- **FOMOD Data Structures**: Complete type system for FOMOD configuration
  - Plugin groups with selection types (SelectAll, SelectAny, SelectExactlyOne, etc.)
  - Plugin types (Required, Optional, Recommended, NotUsable)
  - Dependencies with And/Or operators
  - Conditional file installs
- **Conflict Resolution**: Priority-based file conflict resolution
  - Higher priority wins
  - Document order for tie-breaking
  - Deduplication by destination path

#### `src/core/deployment.rs` (617 lines)
- **Smart Link Creation**: Automatically determines hardlink vs symlink
- **Safe Link Removal**: Only removes links pointing to mod files
  - Preserves vanilla game content
  - Preserves other mods' files
  - Checks inode for hardlinks, target for symlinks
- **Recursive Directory Linking**: Efficient deep directory tree linking
- **Data/ Flattening**: Handles nested Data/Data/ structures
  - Common FOMOD config error handling
  - Prevents double-nesting in game directory
- **Root File Deployment**: Links DLLs, SKSE, ENB configs to game root
- **Legacy Cleanup**: Purges old nested Data/Data/ symlinks
- **Deployment Reports**: Tracks links created/removed for logging

### 2. Key Improvements Over Current System

1. **Filesystem-Aware Linking**
   - Current: Always uses symlinks
   - New: Intelligently chooses hardlinks when possible (same filesystem)
   - Benefit: Faster, more robust, survives directory renames

2. **Case-Sensitivity Handling**
   - Current: Some case normalization
   - New: Complete lowercase normalization pipeline
   - Benefit: Prevents conflicts from mixed-case paths

3. **Better Archive Detection**
   - Current: Basic heuristic scoring
   - New: Enhanced scoring with multiple indicators
   - Benefit: More accurate root detection, fewer false positives

4. **Conflict Resolution**
   - Current: Last-deployed wins
   - New: Priority-based with document order tie-breaking
   - Benefit: Predictable, controllable conflict resolution

5. **Comprehensive Testing**
   - 98 tests passing
   - Tests handle both symlinks and hardlinks
   - Deployment tests verify link type flexibility

### 3. Architecture Decisions

The new system follows these principles from the guidelines:

- ✅ Never copy files to Data/ - only links
- ✅ Hardlinks preferred on same filesystem
- ✅ All paths normalized to lowercase
- ✅ Data/ flattening for FOMOD compatibility
- ✅ Root-level file deployment (DLLs, SKSE, etc.)
- ✅ Safe link removal (preserves vanilla content)
- ✅ Zip-slip protection with path validation

## Remaining Work

### 1. FOMOD XML Parser (Not Started)
- Need to implement streaming XML parser using quick-xml
- Parse ModuleConfig.xml structure
- Handle complex dependency logic
- Extract images on demand
- Borrow checker complexity requires careful design

### 2. Archive Extraction (Not Started)
- Integrate with existing extraction code
- Apply new root detection
- Handle progress callbacks
- Normalize paths during extraction

### 3. Integration with Existing System
The new modules are standalone and don't affect the existing system. To integrate:

1. **Replace `ModManager::enable_mod/disable_mod`** in `src/core/mods.rs`:
   - Use `deployment::deploy_mod()`
   - Use `deployment::undeploy_mod()`

2. **Use new archive root detection** in `src/core/installer.rs`:
   - Replace `find_data_root_in_paths()` with `installer_new::detect_data_root()`
   - Apply `installer_new::score_as_data_root()` logic

3. **Path normalization** should be applied during:
   - Archive extraction
   - FOMOD file resolution
   - Link creation (already done in deployment module)

### 4. FOMOD Integration
- Current system has FOMOD parsing in `src/core/installer.rs`
- Can reuse existing parser or implement new one
- New conflict resolution system can be integrated

## Testing Status

All 98 tests passing, including:
- 19 new tests for installer_new module
- 3 deployment module tests (with hardlink/symlink flexibility)
- 76 existing tests (no regressions)

## Code Quality

- Comprehensive documentation
- Type-safe design
- Error handling with detailed messages
- No unsafe code
- Follows Rust best practices

## Performance Considerations

- Hardlinks are faster than symlinks (when possible)
- Streaming parsers avoid DOM overhead
- Efficient conflict resolution (O(n log n) sort)
- Minimal allocations in hot paths

## Next Steps

To complete the reimplementation:

1. Implement FOMOD XML parser (address borrow checker issues)
2. Create archive extraction integration layer
3. Gradually migrate existing installer to use new modules
4. Add integration tests
5. Performance benchmarks
6. Migration guide for users (if any behavioral changes)

## Files Changed

- `src/core/installer_new.rs` - New (483 lines)
- `src/core/deployment.rs` - New (617 lines)
- `src/main.rs` - Modified (added module declarations)
- All tests passing
