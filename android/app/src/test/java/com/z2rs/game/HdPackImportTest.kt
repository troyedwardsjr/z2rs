package com.z2rs.game

import com.z2rs.game.HdPackImport.EntryPath
import com.z2rs.game.HdPackImport.ImportException
import com.z2rs.game.HdPackImport.Kind
import com.z2rs.game.HdPackImport.PackCheck
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.nio.file.Files
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

class HdPackImportTest {
    private lateinit var tmp: File

    @Before
    fun setUp() {
        tmp = Files.createTempDirectory("hdpack-test").toFile()
    }

    @After
    fun tearDown() {
        tmp.deleteRecursively()
    }

    private fun zip(vararg entries: Pair<String, ByteArray?>): ByteArray {
        val bytes = ByteArrayOutputStream()
        ZipOutputStream(bytes).use { z ->
            for ((name, data) in entries) {
                z.putNextEntry(ZipEntry(name))
                if (data != null) z.write(data)
                z.closeEntry()
            }
        }
        return bytes.toByteArray()
    }

    private fun extract(zipBytes: ByteArray, limits: HdPackImport.Limits = HdPackImport.Limits(), free: Long = Long.MAX_VALUE): List<String> {
        val dest = File(tmp, "out").apply { mkdirs() }
        return HdPackImport.extractZip(
            ByteArrayInputStream(zipBytes),
            dest,
            limits,
            HdPackImport.Budget(limits, freeBytes = { free }),
        )
    }

    private fun expectKind(kind: Kind, block: () -> Unit) {
        try {
            block()
            fail("expected $kind")
        } catch (e: ImportException) {
            assertEquals(kind, e.kind)
        }
    }

    @Test
    fun entryPathsAreNormalised() {
        assertEquals(EntryPath.Keep("pack.json"), HdPackImport.checkEntryPath("pack.json"))
        assertEquals(EntryPath.Keep("my pack/sheets/p5.png"), HdPackImport.checkEntryPath("my pack\\sheets\\.\\p5.png"))
        assertEquals(EntryPath.Keep("a/b.png"), HdPackImport.checkEntryPath("./a//b.png"))
        assertEquals(EntryPath.Skip, HdPackImport.checkEntryPath("sheets/"))
        assertEquals(EntryPath.Skip, HdPackImport.checkEntryPath("./"))
    }

    @Test
    fun zipSlipPathsAreUnsafe() {
        for (bad in listOf(
            "../evil.png", "sheets/../../evil", "a\\..\\..\\b", "/etc/passwd", "\\abs", "C:/x.png", "c:\\x", "a\u0000b",
        )) {
            assertTrue(bad, HdPackImport.checkEntryPath(bad) is EntryPath.Unsafe)
        }
        assertTrue(HdPackImport.checkEntryPath("x/".repeat(20) + "f", maxDepth = 16) is EntryPath.Unsafe)
        assertTrue(HdPackImport.checkEntryPath("a".repeat(256)) is EntryPath.Unsafe)
    }

    @Test
    fun junkIsSkipped() {
        for (junk in listOf("__MACOSX/pack/._pack.json", ".DS_Store", "pack/.git/config", "sheets/Thumbs.db", "desktop.ini", "._x")) {
            assertEquals(junk, EntryPath.Skip, HdPackImport.checkEntryPath(junk))
        }
    }

    @Test
    fun packRootIsTheShallowestManifest() {
        assertEquals("", HdPackImport.findPackRoot(listOf("sheets/a.png", "pack.json", "x/pack.json")))
        assertEquals("My Pack/", HdPackImport.findPackRoot(listOf("My Pack/sheets/a.png", "My Pack/pack.json")))
        assertEquals("a/", HdPackImport.findPackRoot(listOf("b/pack.json", "a/pack.json", "a/z/pack.json")))
        assertNull(HdPackImport.findPackRoot(listOf("sheets/a.png", "notpack.json", "pack.json.bak")))
        assertNull(HdPackImport.findPackRoot(emptyList()))
    }

    @Test
    fun extractsAWrappedPack() {
        val kept = extract(
            zip(
                "My Pack/" to null,
                "My Pack/pack.json" to "{}".toByteArray(),
                "My Pack/sheets/p5.png" to ByteArray(1000) { it.toByte() },
                "__MACOSX/My Pack/._pack.json" to ByteArray(10),
            ),
        )
        assertEquals(listOf("My Pack/pack.json", "My Pack/sheets/p5.png"), kept)
        assertEquals("My Pack/", HdPackImport.findPackRoot(kept))
        val out = File(tmp, "out")
        assertEquals("{}", File(out, "My Pack/pack.json").readText())
        assertEquals(1000L, File(out, "My Pack/sheets/p5.png").length())
        assertFalse(File(out, "__MACOSX").exists())
    }

    @Test
    fun aZipSlipEntryRefusesTheWholeZip() {
        // ZipOutputStream writes the name as given; ZipInputStream on the JVM hands it back.
        expectKind(Kind.UNSAFE_PATH) { extract(zip("pack.json" to "{}".toByteArray(), "../../escape.txt" to "x".toByteArray())) }
        assertFalse(File(tmp, "escape.txt").exists())
        assertFalse(File(tmp.parentFile, "escape.txt").exists())
    }

    @Test
    fun androidsOwnPathValidatorCountsAsZipSlip() {
        val slip = HdPackImport.zipFailure("Invalid zip entry path: ../../evil.txt")
        assertEquals(Kind.UNSAFE_PATH, slip.kind)
        assertEquals("../../evil.txt", slip.detail)
        assertEquals(Kind.NOT_A_ZIP, HdPackImport.zipFailure("invalid LOC header (bad signature)").kind)
        assertEquals(Kind.NOT_A_ZIP, HdPackImport.zipFailure(null).kind)
    }

    @Test
    fun resolveInsideRefusesEscapes() {
        val root = File(tmp, "root").apply { mkdirs() }
        assertEquals(File(root, "a/b").canonicalFile, HdPackImport.resolveInside(root, "a/b"))
        expectKind(Kind.UNSAFE_PATH) { HdPackImport.resolveInside(root, "../x") }
        expectKind(Kind.UNSAFE_PATH) { HdPackImport.resolveInside(root, "") }
    }

    @Test
    fun limitsAreEnforcedOnWhatIsWritten() {
        val small = HdPackImport.Limits(maxFiles = 2, maxFileBytes = 100, maxTotalBytes = 150, reserveBytes = 10)
        expectKind(Kind.TOO_MANY_FILES) { extract(zip("a" to ByteArray(1), "b" to ByteArray(1), "c" to ByteArray(1)), small) }
        // Highly compressible: tiny in the zip, too big unpacked.
        expectKind(Kind.FILE_TOO_BIG) { extract(zip("big.png" to ByteArray(10_000)), small) }
        expectKind(Kind.TOTAL_TOO_BIG) { extract(zip("a" to ByteArray(90), "b" to ByteArray(90)), small) }
        expectKind(Kind.NO_SPACE) { extract(zip("a" to ByteArray(1)), small, free = 5) }
        assertEquals(listOf("a", "b"), extract(zip("a" to ByteArray(70), "b" to ByteArray(70)), small))
    }

    @Test
    fun notAZip() {
        expectKind(Kind.NOT_A_ZIP) { extract("this is not a zip file".toByteArray()) }
        expectKind(Kind.NOT_A_ZIP) { extract(ByteArray(0)) }
    }

    @Test
    fun progressCanCancel() {
        val dest = File(tmp, "out").apply { mkdirs() }
        val limits = HdPackImport.Limits()
        var calls = 0
        val budget = HdPackImport.Budget(limits, onProgress = { _, _ ->
            calls++
            throw IllegalStateException("cancelled")
        })
        try {
            HdPackImport.extractZip(ByteArrayInputStream(zip("a" to ByteArray(10))), dest, limits, budget)
            fail("progress hook should stop the import")
        } catch (e: IllegalStateException) {
            assertEquals(1, calls)
        }
    }

    @Test
    fun storeCommitsAndRemoves() {
        val store = HdPackStore(tmp)
        File(store.freshStaging(), "p/pack.json").apply { parentFile!!.mkdirs(); writeText("1") }
        store.commit()
        assertTrue(store.isInstalled("p/"))
        assertEquals("1", File(store.packDir("p/"), "pack.json").readText())
        assertFalse(store.stagingDir.exists())

        // A second import replaces the first only on commit.
        File(store.freshStaging(), "pack.json").writeText("2")
        assertTrue(store.isInstalled("p/"))
        store.commit()
        assertTrue(store.isInstalled(""))
        assertFalse(store.isInstalled("p/"))

        File(store.freshStaging(), "pack.json").writeText("3")
        store.discardStaging()
        assertEquals("2", File(store.packDir(""), "pack.json").readText())

        store.remove()
        assertFalse(store.isInstalled(""))
        assertFalse(store.installDir.exists())
    }

    @Test
    fun checkReplies() {
        assertEquals(PackCheck.Ok("My Pack", 4, 812, 2), HdPackImport.parseCheckReply("ok\tMy Pack\t4\t812\t2"))
        assertEquals(PackCheck.Bad("pack.json: \"version\" is 2"), HdPackImport.parseCheckReply("error\tpack.json: \"version\" is 2"))
        assertEquals(PackCheck.Unavailable, HdPackImport.parseCheckReply(null))
        assertTrue(HdPackImport.parseCheckReply("ok\tx\t1") is PackCheck.Bad)
        assertTrue(HdPackImport.parseCheckReply("ok\tx\tA\t1\t1") is PackCheck.Bad)
        assertTrue(HdPackImport.parseCheckReply("") is PackCheck.Bad)
    }
}
