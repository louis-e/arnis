# The walker's next chance into #r, from its own sequence.
scoreboard players operation @s arnis.wr *= #lcg_a arnis.walk
scoreboard players operation @s arnis.wr += #lcg_c arnis.walk
scoreboard players operation #r arnis.walk = @s arnis.wr
scoreboard players operation #r arnis.walk /= #65536 arnis.walk
