execute store result score #r arnis.ds run random value 1..3
execute if score #r arnis.ds matches 1 rotated ~50 0 summon marker run function arnis:disaster/quake/twig
execute if score #r arnis.ds matches 2 rotated ~-50 0 summon marker run function arnis:disaster/quake/twig
